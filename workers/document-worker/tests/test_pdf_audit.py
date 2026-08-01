"""Tests for `pdf.audit` handler (Etapa 5 PR 3, D-PDF5/D-PDF6 do ADR-0021).

**Regra do projeto (REGRAS-DO-PROJETO.md §1.3):** mudanca de
comportamento/contrato/schema do `pdf.audit` exige atualizacao
deste arquivo no mesmo commit. A inversa tambem: bump de regra
aqui exige bump do `AUDIT_RULES_VERSION` em `document-worker.py`
no mesmo commit (D-AUDIT-1).

**Como rodar (CI ainda nao roda pytest - ver README do worker):**
    cd workers/document-worker
    .\\runtime\\python.exe tests/test_pdf_audit.py
    # exit 0 = todos verde, exit 1 = algum falhou

**O que este arquivo NAO faz:** nao roda o caminho de IPC
completo (worker spawna named pipe, app conecta). Os testes
chamam o handler `handle_pdf_audit` direto, com PDFs sinteticos
construidos com pikepdf. O caminho IPC end-to-end tem testes
Rust em `crates/document-kits/tests/e2e_docs_generate_pdf.rs`
(E2E vertical 1 do §33 do PROMPT MESTRE).
"""

from __future__ import annotations

import importlib.util
import os
import sys
import tempfile
import traceback
from pathlib import Path


# ---------------------------------------------------------------------------
# Setup: importa o document-worker
# ---------------------------------------------------------------------------

WORKER_DIR = Path(__file__).resolve().parent.parent
WORKER_FILE = WORKER_DIR / "document-worker.py"
ICC_PATH = WORKER_DIR / "runtime" / "icc" / "sRGB.icc"
TMP_DIR = Path(tempfile.gettempdir())


def _load_dw():
    """Carrega `document-worker.py` (nome com hifen) via spec."""
    spec = importlib.util.spec_from_file_location(
        "document_worker", str(WORKER_FILE)
    )
    if spec is None or spec.loader is None:
        raise RuntimeError(f"nao consegui carregar {WORKER_FILE}")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


dw = _load_dw()
import pikepdf  # noqa: E402  - apos carregar dw, pra ter o runtime deps ok


# ---------------------------------------------------------------------------
# Helpers de construcao de PDF sintetico
# ---------------------------------------------------------------------------


def _save(pdf: pikepdf.Pdf, name: str) -> Path:
    p = TMP_DIR / f"audit_test_{name}.pdf"
    pdf.save(str(p))
    return p


def _minimal_pdf() -> pikepdf.Pdf:
    """PDF baseline valido: 1 pagina, DocInfo populado, sem fontes."""
    pdf = pikepdf.Pdf.new()
    page = pikepdf.Page(
        pikepdf.Dictionary(
            Type=pikepdf.Name.Page,
            MediaBox=[0, 0, 595, 842],
        )
    )
    pdf.pages.append(page)
    pdf.docinfo["/Title"] = "Test"
    pdf.docinfo["/Author"] = "Test"
    pdf.docinfo["/Producer"] = "Test"
    pdf.docinfo["/Creator"] = "Test"
    return pdf


def _add_a2b_metadata(pdf: pikepdf.Pdf) -> None:
    """Seta XMP com pdfaid:part=2 e conformance=B."""
    with pdf.open_metadata() as xmp:
        xmp["pdfaid:part"] = "2"
        xmp["pdfaid:conformance"] = "B"


def _add_output_intent_with_icc(pdf: pikepdf.Pdf) -> None:
    """Adiciona /OutputIntents[0] referenciando o ICC sRGB gerado pelo bootstrap.

    **Importante:** seta `Filter = None` no Stream. Sem isso, pikepdf
    aplica FlateDecode no save e a auditoria (que le bytes brutos
    via `get_data()`) pega o stream comprimido em vez do ICC.
    PDF/A-2B espera o ICC raw, nao comprimido - ICC ja tem seu
    proprio profile header.
    """
    if not ICC_PATH.is_file():
        raise RuntimeError(
            f"ICC nao encontrado em {ICC_PATH} - rode bootstrap.ps1 primeiro"
        )
    icc_bytes = ICC_PATH.read_bytes()
    icc_stream = pikepdf.Stream(pdf, icc_bytes)
    # PDF/A-2B espera o ICC raw, nao comprimido. ICC ja tem seu
    # proprio profile header. `del /Filter` em vez de `= None`
    # porque pikepdf recusa set None (precisa `del`).
    if "/Filter" in icc_stream:
        del icc_stream["/Filter"]
    pdf.Root.OutputIntents = [
        pikepdf.Dictionary(
            Type=pikepdf.Name.OutputIntent,
            S=pikepdf.Name.GTS_PDFA1,
            DestOutputProfile=icc_stream,
            Info="sRGB IEC61966-2.1",
        )
    ]


# ---------------------------------------------------------------------------
# Test runner minimo (sem pytest - nao esta no stack do Frederico)
# ---------------------------------------------------------------------------


_RESULTS: list[tuple[str, str, str | None]] = []  # (name, status, error)


def _run(name: str, fn) -> None:
    try:
        fn()
        _RESULTS.append((name, "OK", None))
        print(f"  [OK]   {name}")
    except AssertionError as e:
        _RESULTS.append((name, "FAIL", str(e)))
        print(f"  [FAIL] {name}: {e}")
    except Exception as e:
        tb = traceback.format_exc()
        _RESULTS.append((name, "ERROR", tb))
        print(f"  [ERR]  {name}: {type(e).__name__}: {e}")


# ---------------------------------------------------------------------------
# Tests - baseline (rodados SEM pdfa: opt-in)
# ---------------------------------------------------------------------------


def test_baseline_ok():
    """PDF minimo valido passa todos os 5 checks baseline."""
    pdf = _minimal_pdf()
    p = _save(pdf, "baseline_ok")
    r = dw.handle_pdf_audit({"path": str(p), "kind": "structural"})
    assert r["ok"] is True, f"esperado ok=True, veio {r}"
    assert r["coverage"] == "full"
    assert r["rules_version"] == "0.1.0"
    assert ":0.1.0" in r["cache_key"], f"cache_key sem rules_version: {r['cache_key']}"
    assert len(r["checks"]) == 5, f"esperado 5 checks, veio {len(r['checks'])}: {r['checks']}"
    checks = {c["check"] for c in r["checks"]}
    assert checks == {"n_pages", "docinfo_populated", "fonts_embedded",
                       "no_external_refs", "no_encryption"}


def test_baseline_n_pages_consistency():
    """Cross-check de n_pages com pages_rendered do write."""
    pdf = _minimal_pdf()
    pdf.pages.append(pikepdf.Page(pikepdf.Dictionary(
        Type=pikepdf.Name.Page, MediaBox=[0, 0, 595, 842]
    )))
    p = _save(pdf, "baseline_n_pages")
    r = dw.handle_pdf_audit({
        "path": str(p), "kind": "structural", "expected_pages": 99,
    })
    assert r["ok"] is False
    assert any(f["check"] == "n_pages_consistency" for f in r["failed"])


def test_baseline_no_docinfo():
    """PDF sem DocInfo populado falha com docinfo_field."""
    pdf = pikepdf.Pdf.new()
    pdf.pages.append(pikepdf.Page(pikepdf.Dictionary(
        Type=pikepdf.Name.Page, MediaBox=[0, 0, 595, 842]
    )))
    # Nao popula docinfo.
    p = _save(pdf, "baseline_no_docinfo")
    r = dw.handle_pdf_audit({"path": str(p), "kind": "structural"})
    assert r["ok"] is False
    assert any(f["check"] == "docinfo_field" for f in r["failed"])


def test_baseline_encrypted():
    """PDF cifrado falha com no_encryption (PDF/A-2B proibe)."""
    pdf = _minimal_pdf()
    p = TMP_DIR / "audit_test_baseline_encrypted.pdf"
    # pikepdf 10.x exige AES para encrypt metadata; aes=False nao
    # e permitido. PDF/A-2B rejeita cifragem de qualquer jeito -
    # o handler mapeia PasswordError pra `no_encryption` no `failed`.
    pdf.save(
        str(p),
        encryption=pikepdf.Encryption(user="u", owner="o", aes=True, R=4),
    )
    r = dw.handle_pdf_audit({"path": str(p), "kind": "structural"})
    assert r["ok"] is False
    assert r["code"] == "pdf_audit_structural_failed"
    assert any(f["check"] == "no_encryption" for f in r["failed"])


def test_baseline_external_uri():
    """PDF com Link annotation apontando pra URL externa falha."""
    pdf = _minimal_pdf()
    page = pdf.pages[0]
    page.obj["/Annots"] = [
        pikepdf.Dictionary(
            Type=pikepdf.Name.Annot,
            Subtype=pikepdf.Name.Link,
            Rect=[0, 0, 100, 100],
            A=pikepdf.Dictionary(
                Type=pikepdf.Name.Action,
                S=pikepdf.Name.URI,
                URI="https://example.com/external",
            ),
        )
    ]
    p = _save(pdf, "baseline_external_uri")
    r = dw.handle_pdf_audit({"path": str(p), "kind": "structural"})
    assert r["ok"] is False
    assert any(f["check"] == "external_uri" for f in r["failed"])


def test_baseline_external_embedded_file():
    """PDF com /EmbeddedFiles /F (path absoluto) falha."""
    pdf = _minimal_pdf()
    pdf.Root["/Names"] = pikepdf.Dictionary(
        EmbeddedFiles=pikepdf.Dictionary(
            Names=[
                "leaked",
                pikepdf.Dictionary(
                    Type=pikepdf.Name.Filespec,
                    F="/etc/passwd",
                ),
            ]
        )
    )
    p = _save(pdf, "baseline_external_embedded")
    r = dw.handle_pdf_audit({"path": str(p), "kind": "structural"})
    assert r["ok"] is False
    assert any(f["check"] == "external_embedded_file" for f in r["failed"])


def test_baseline_corrupted():
    """PDF corrompido (bytes nao-PDF) falha com pdf_open."""
    p = TMP_DIR / "audit_test_corrupted.pdf"
    p.write_bytes(b"this is not a PDF, just some random bytes")
    r = dw.handle_pdf_audit({"path": str(p), "kind": "structural"})
    assert r["ok"] is False
    assert r["code"] == "pdf_audit_structural_failed"
    assert any(f["check"] == "pdf_open" for f in r["failed"])


# ---------------------------------------------------------------------------
# Tests - PDF/A-2B opt-in (D-PDF5)
# ---------------------------------------------------------------------------


def test_pdfa2b_ok():
    """PDF com OutputIntent (ICC) + XMP pdfaid = 2/B passa tudo.

    **Ordem importa:** seta docinfo DEPOIS de XMP/output_intent.
    Pikepdf reescreve docinfo a partir do XMP no save, e se
    Title/Author/Creator nao tiverem contraparte em XMP, some.
    Setar docinfo por ultimo garante que o PDF final tem os 4.
    """
    pdf = pikepdf.Pdf.new()
    pdf.pages.append(pikepdf.Page(pikepdf.Dictionary(
        Type=pikepdf.Name.Page, MediaBox=[0, 0, 595, 842]
    )))
    _add_a2b_metadata(pdf)
    _add_output_intent_with_icc(pdf)
    # Set DEPOIS do XMP/output_intent - pikepdf nao vai sobrescrever.
    pdf.docinfo["/Title"] = "Test"
    pdf.docinfo["/Author"] = "Test"
    pdf.docinfo["/Producer"] = "Test"
    pdf.docinfo["/Creator"] = "Test"
    p = _save(pdf, "pdfa2b_ok")
    r = dw.handle_pdf_audit({
        "path": str(p), "kind": "structural", "pdfa": "pdfa_2b",
    })
    assert r["ok"] is True, f"esperado ok=True, veio {r}"
    pdfa2b_checks = [c for c in r["checks"] if c["check"] == "pdfa2b_compliance"]
    assert len(pdfa2b_checks) == 1


def test_pdfa2b_missing_output_intent():
    """pdfa: pdfa_2b sem OutputIntent falha."""
    pdf = _minimal_pdf()
    _add_a2b_metadata(pdf)
    p = _save(pdf, "pdfa2b_no_oi")
    r = dw.handle_pdf_audit({
        "path": str(p), "kind": "structural", "pdfa": "pdfa_2b",
    })
    assert r["ok"] is False
    assert any(f["check"] == "pdfa2b_output_intent" for f in r["failed"])


def test_pdfa2b_missing_xmp():
    """pdfa: pdfa_2b sem os campos pdfaid certos falha.

    **Nota pikepdf 10.x:** o `save()` recria /Metadata
    automaticamente se nao existir (XMP default vazio). Entao
    o teste constroi um PDF com XMP existente mas SEM os campos
    `pdfaid:part` e `pdfaid:conformance`. A auditoria pega
    via `pdfa2b_xmp_part` / `pdfa2b_xmp_conformance` (o mesmo
    grupo de checks do "missing" - o efeito final pro caller e
    o mesmo: PDF/A-2B opt-in falha). Real PDF com XMP
    completamente ausente seria pego pelo check
    `pdfa2b_xmp_present`; mantemos esse check no audit pra
    cobrir o caso real (Producer que nao cria XMP nenhum).
    """
    pdf = _minimal_pdf()
    _add_output_intent_with_icc(pdf)
    # Nao chama _add_a2b_metadata. pikepdf pode recriar XMP
    # vazio no save - o audit ainda pega via part/conformance.
    p = _save(pdf, "pdfa2b_no_xmp")
    r = dw.handle_pdf_audit({
        "path": str(p), "kind": "structural", "pdfa": "pdfa_2b",
    })
    assert r["ok"] is False
    failed_checks = {f["check"] for f in r["failed"]}
    # Aceita qualquer falha do grupo XMP (present / part /
    # conformance). O efeito pro caller e o mesmo.
    xmp_failures = failed_checks & {
        "pdfa2b_xmp_present", "pdfa2b_xmp_part", "pdfa2b_xmp_conformance",
    }
    assert xmp_failures, (
        f"esperado alguma falha pdfa2b_xmp_*, veio {failed_checks}"
    )


def test_pdfa2b_wrong_xmp_part():
    """XMP com pdfaid:part=1 (em vez de 2) falha."""
    pdf = _minimal_pdf()
    with pdf.open_metadata() as xmp:
        xmp["pdfaid:part"] = "1"  # errado: 2B exige 2
        xmp["pdfaid:conformance"] = "B"
    _add_output_intent_with_icc(pdf)
    p = _save(pdf, "pdfa2b_wrong_part")
    r = dw.handle_pdf_audit({
        "path": str(p), "kind": "structural", "pdfa": "pdfa_2b",
    })
    assert r["ok"] is False
    assert any(f["check"] == "pdfa2b_xmp_part" for f in r["failed"])


def test_pdfa2b_wrong_xmp_conformance():
    """XMP com pdfaid:conformance=U (em vez de B) falha."""
    pdf = _minimal_pdf()
    with pdf.open_metadata() as xmp:
        xmp["pdfaid:part"] = "2"
        xmp["pdfaid:conformance"] = "U"  # errado: 2B exige B
    _add_output_intent_with_icc(pdf)
    p = _save(pdf, "pdfa2b_wrong_conf")
    r = dw.handle_pdf_audit({
        "path": str(p), "kind": "structural", "pdfa": "pdfa_2b",
    })
    assert r["ok"] is False
    assert any(f["check"] == "pdfa2b_xmp_conformance" for f in r["failed"])


def test_pdfa2b_missing_icc():
    """OutputIntent sem /DestOutputProfile falha."""
    pdf = _minimal_pdf()
    _add_a2b_metadata(pdf)
    pdf.Root.OutputIntents = [
        pikepdf.Dictionary(
            Type=pikepdf.Name.OutputIntent,
            S=pikepdf.Name.GTS_PDFA1,
            # No /DestOutputProfile
            Info="sRGB",
        )
    ]
    p = _save(pdf, "pdfa2b_no_icc")
    r = dw.handle_pdf_audit({
        "path": str(p), "kind": "structural", "pdfa": "pdfa_2b",
    })
    assert r["ok"] is False
    assert any(f["check"] == "pdfa2b_icc_profile" for f in r["failed"])


def test_pdfa2b_bad_icc():
    """OutputIntent com ICC invalido (sem acsp) falha."""
    pdf = _minimal_pdf()
    _add_a2b_metadata(pdf)
    pdf.Root.OutputIntents = [
        pikepdf.Dictionary(
            Type=pikepdf.Name.OutputIntent,
            S=pikepdf.Name.GTS_PDFA1,
            DestOutputProfile=pikepdf.Stream(pdf, b"not a valid ICC profile"),
            Info="bogus",
        )
    ]
    p = _save(pdf, "pdfa2b_bad_icc")
    r = dw.handle_pdf_audit({
        "path": str(p), "kind": "structural", "pdfa": "pdfa_2b",
    })
    assert r["ok"] is False
    failed_checks = {f["check"] for f in r["failed"]}
    assert "pdfa2b_icc_valid" in failed_checks, (
        f"esperado pdfa2b_icc_valid, veio {failed_checks}"
    )


def test_pdfa2b_javascript_openaction():
    """PDF com /OpenAction /JS falha (PDF/A-2B proibe JS)."""
    pdf = _minimal_pdf()
    _add_a2b_metadata(pdf)
    _add_output_intent_with_icc(pdf)
    pdf.Root.OpenAction = pikepdf.Dictionary(
        Type=pikepdf.Name.Action,
        S=pikepdf.Name.JavaScript,
        JS="app.alert('boom');",
    )
    p = _save(pdf, "pdfa2b_js_open")
    r = dw.handle_pdf_audit({
        "path": str(p), "kind": "structural", "pdfa": "pdfa_2b",
    })
    assert r["ok"] is False
    assert any(f["check"] == "pdfa2b_no_javascript" for f in r["failed"])


def test_pdfa2b_javascript_names():
    """PDF com /Names/JavaScript falha."""
    pdf = _minimal_pdf()
    _add_a2b_metadata(pdf)
    _add_output_intent_with_icc(pdf)
    pdf.Root.Names = pikepdf.Dictionary(
        JavaScript=pikepdf.Dictionary(
            Names=["hack", pikepdf.Dictionary(
                Type=pikepdf.Name.Action,
                S=pikepdf.Name.JavaScript,
                JS="x();",
            )]
        )
    )
    p = _save(pdf, "pdfa2b_js_names")
    r = dw.handle_pdf_audit({
        "path": str(p), "kind": "structural", "pdfa": "pdfa_2b",
    })
    assert r["ok"] is False
    assert any(f["check"] == "pdfa2b_no_javascript" for f in r["failed"])


# ---------------------------------------------------------------------------
# Tests - error path / contract
# ---------------------------------------------------------------------------


def test_kind_unsupported():
    """kind='visual' retorna audit_kind_unsupported (PR 4 cobre)."""
    pdf = _minimal_pdf()
    p = _save(pdf, "kind_unsupported")
    r = dw.handle_pdf_audit({
        "path": str(p), "kind": "visual",
    })
    assert r["ok"] is False
    assert r["code"] == "audit_kind_unsupported"


def test_missing_path():
    """payload sem path retorna invalid_input."""
    r = dw.handle_pdf_audit({"path": "", "kind": "structural"})
    assert r["ok"] is False
    assert r["code"] == "invalid_input"


def test_path_traversal():
    """path com '..' e barrido pelo validate_path (path_traversal)."""
    r = dw.handle_pdf_audit({
        "path": "../etc/passwd", "kind": "structural",
    })
    assert r["ok"] is False
    assert r["code"] == "path_traversal"


# ---------------------------------------------------------------------------
# Test runner
# ---------------------------------------------------------------------------


def main() -> int:
    if not ICC_PATH.is_file():
        print(f"ERRO: ICC nao encontrado em {ICC_PATH}")
        print("Rode bootstrap.ps1 primeiro.")
        return 2

    tests = [
        ("baseline_ok", test_baseline_ok),
        ("baseline_n_pages_consistency", test_baseline_n_pages_consistency),
        ("baseline_no_docinfo", test_baseline_no_docinfo),
        ("baseline_encrypted", test_baseline_encrypted),
        ("baseline_external_uri", test_baseline_external_uri),
        ("baseline_external_embedded_file", test_baseline_external_embedded_file),
        ("baseline_corrupted", test_baseline_corrupted),
        ("pdfa2b_ok", test_pdfa2b_ok),
        ("pdfa2b_missing_output_intent", test_pdfa2b_missing_output_intent),
        ("pdfa2b_missing_xmp", test_pdfa2b_missing_xmp),
        ("pdfa2b_wrong_xmp_part", test_pdfa2b_wrong_xmp_part),
        ("pdfa2b_wrong_xmp_conformance", test_pdfa2b_wrong_xmp_conformance),
        ("pdfa2b_missing_icc", test_pdfa2b_missing_icc),
        ("pdfa2b_bad_icc", test_pdfa2b_bad_icc),
        ("pdfa2b_javascript_openaction", test_pdfa2b_javascript_openaction),
        ("pdfa2b_javascript_names", test_pdfa2b_javascript_names),
        ("kind_unsupported", test_kind_unsupported),
        ("missing_path", test_missing_path),
        ("path_traversal", test_path_traversal),
    ]

    print(f"=== {len(tests)} testes do pdf.audit (handler Python, D-PDF5/D-PDF6) ===")
    for name, fn in tests:
        _run(name, fn)

    ok = sum(1 for _, s, _ in _RESULTS if s == "OK")
    fail = sum(1 for _, s, _ in _RESULTS if s == "FAIL")
    err = sum(1 for _, s, _ in _RESULTS if s == "ERROR")
    print()
    print(f"Total: {len(_RESULTS)}  OK: {ok}  FAIL: {fail}  ERROR: {err}")
    if fail or err:
        print("FALHAS:")
        for name, status, err in _RESULTS:
            if status != "OK":
                print(f"  [{status}] {name}")
                if err:
                    for line in err.splitlines()[-5:]:
                        print(f"    {line}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
