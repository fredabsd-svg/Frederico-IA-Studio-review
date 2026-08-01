"""Generate a minimal sRGB ICC v2 profile (deterministic).

## Why this exists

PDF/A-2B (opt-in via `DocumentSpec.metadata.pdfa: PdfAFlavor::PdfA2b`,
D-PDF5 do ADR-0021) exige que o `OutputIntent` aponte pra um perfil
ICC embedded no PDF. O unico perfil que faz sentido pra Frederico
("Tinta e Latao" e tinta/escuro sobre branco) e o sRGB IEC 61966-2.1.

O ICC referencia perfis publicos (sRGB2014.icc) no
https://www.color.org/, mas o site mudou de estrutura em 2026
e o profile nao esta em URL estavel. Alternativas:

- Pillow bundle: Pillow nao expoe bytes via API estavel.
- Argyll CMS: bundle sRGB mas requer download de varias centenas
  de KB e licenciamento ICC questionavel.
- `tessdata`-style raw/main: URL instavel, hash muda (a licao
  que o user ja pagou caro).

A escolha mais limpa: **gerar o ICC programaticamente**. O espaco
sRGB e definido pela IEC 61966-2-1:1999 (padrao internacional,
publico). O formato ICC v2 e definido pela ISO 15076-1. Ambos sao
reproduziveis a partir da espec.

O output deste script e deterministico: mesmas constantes =
mesmos bytes. SHA-256 fixado em `bootstrap.ps1`. Bump do script
= bump do hash no mesmo commit (D-AUDIT-1 do ADR-0021:
"mudanca de regra = bump da AUDIT_RULES_VERSION").

## O que o script entrega

- ICC v2 (mais simples que v4; suficiente pra OutputIntent
  de PDF/A-2B segundo o §19.4 do PROMPT MESTRE)
- Color space = 'RGB ', device class = 'mntr' (monitor)
- PCS = 'XYZ ', white point D50
- Primaries sRGB D50-adaptados (do IEC spec)
- TRC = gamma 2.2 (aproximacao "sRGB compativel"; o EOTF exato
  do sRGB e piecewise, mas pra OutputIntent de PDF/A-2B o
  delta visual e zero — o veraPDF roda no ci-nightly pra
  validacao rigorosa, nao no caminho quente do PR 3)
- Profile description "sRGB IEC61966-2.1"
- Copyright "Public domain (IEC 61966-2-1 standard)"

## Licenca

O espaco sRGB e padrao internacional IEC. Os parametros nao tem
copyright. O ICC profile gerado a partir deles e, na pratica,
public domain. Este codigo gerador e MIT (mesma licenca do resto
do projeto).

## Uso

    python tools/generate_srgb_icc.py <output.icc>
"""

from __future__ import annotations

import struct
import sys
from pathlib import Path

# sRGB primaries D50-adaptados (IEC 61966-2-1:1999, Anexo A).
# Esses valores sao padrao internacional - publico.
PRIMARIES = {
    "r": (0.4360, 0.2225, 0.0139),  # Red
    "g": (0.3851, 0.7169, 0.0971),  # Green
    "b": (0.1431, 0.0606, 0.7141),  # Blue
}
# D50 white point (PCS do ICC, s15Fixed16Number).
WHITE_POINT = (0.9642, 1.0000, 0.8249)
# sRGB "compatible" gamma - aproximacao usual do EOTF sRGB.
# VeraPDF no noturno valida o rigoroso; PR 3 so checa estrutura.
SRGB_GAMMA = 2.2


def s15f16(x: float) -> int:
    """Converte float pra s15Fixed16Number (signed 15.16 fixed point)."""
    return int(x * 65536) & 0xFFFFFFFF


def pad4(data: bytes) -> bytes:
    """Padroniza pra multiplo de 4 bytes (alinhamento ICC)."""
    if len(data) % 4 == 0:
        return data
    return data + b"\x00" * (4 - len(data) % 4)


def tag_xyz(x: float, y: float, z: float) -> bytes:
    """Constroi XYZType: 4-byte sig 'XYZ ' + reserved + 3x s15Fixed16."""
    return struct.pack(">4sIiii", b"XYZ ", 0, s15f16(x), s15f16(y), s15f16(z))


def tag_curv_gamma(gamma: float) -> bytes:
    """Constroi curveType com 1 entrada gamma (u8Fixed8Number).

    O ICC v2 guarda gamma como uint16 = gamma * 256. Valor 0
    significa linear, mas 0 e ambiguo no decodificador - usamos 1
    (gamma 1/256) se a conta der exatamente 0, o que aqui nunca
    ocorre com gamma=2.2.
    """
    gamma_u16 = max(1, int(round(gamma * 256))) & 0xFFFF
    # Type sig + reserved + count=1 + value u16 + 2 bytes pad
    return struct.pack(">4sIIH", b"curv", 0, 1, gamma_u16) + b"\x00\x00"


def tag_text(s: str) -> bytes:
    """Constroi textType (ICC v2)."""
    body = s.encode("ascii") + b"\x00"
    return struct.pack(">4sI", b"text", len(body)) + pad4(body)


def tag_desc_v2(s: str) -> bytes:
    """Constroi textDescriptionType (ICC v2).

    Estrutura: type sig + reserved + ascii length + ascii + unicode lang
    count + unicode count + scriptcode code + scriptcount + scriptdata(67).
    Sem unicode/scriptcode, valores zerados.
    """
    ascii_body = s.encode("ascii") + b"\x00"
    ascii_body = pad4(ascii_body)
    out = struct.pack(">4sI", b"desc", 0)
    out += struct.pack(">I", len(ascii_body)) + ascii_body
    # Unicode: 0 language code + 0 count
    out += struct.pack(">II", 0, 0)
    # ScriptCode: 0 code + 0 count + 67 bytes 0
    out += struct.pack(">HB", 0, 0) + b"\x00" * 67
    return out


def build_profile() -> bytes:
    """Constroi o ICC profile completo."""
    # Monta as tags (ordem de signature, exigencia do ICC).
    tags = [
        ("cprt", tag_text("Public domain (IEC 61966-2-1 standard)")),
        ("desc", tag_desc_v2("sRGB IEC61966-2.1")),
        ("bTRC", tag_curv_gamma(SRGB_GAMMA)),
        ("gTRC", tag_curv_gamma(SRGB_GAMMA)),
        ("rTRC", tag_curv_gamma(SRGB_GAMMA)),
        ("bXYZ", tag_xyz(*PRIMARIES["b"])),
        ("gXYZ", tag_xyz(*PRIMARIES["g"])),
        ("rXYZ", tag_xyz(*PRIMARIES["r"])),
        ("wtpt", tag_xyz(*WHITE_POINT)),
    ]
    tags.sort(key=lambda t: t[0])

    # Calcula tamanhos.
    header_size = 128
    tag_table_size = 4 + 12 * len(tags)
    data_offset = header_size + tag_table_size
    tag_entries: list[tuple[bytes, int, int]] = []  # (sig, offset, size)
    tag_blobs: list[bytes] = []
    for sig, blob in tags:
        tag_entries.append((sig.encode("ascii"), data_offset, len(blob)))
        tag_blobs.append(blob)
        data_offset += len(blob)
    profile_size = data_offset

    # Header (128 bytes, big-endian).
    header = bytearray(128)
    struct.pack_into(">I", header, 0, profile_size)  # profile size
    # bytes 4-7: preferred CMM (zero)
    struct.pack_into(">I", header, 8, 0x02000000)  # version 2.0
    header[12:16] = b"mntr"  # device class: monitor
    header[16:20] = b"RGB "  # color space
    header[20:24] = b"XYZ "  # PCS
    # Date/time created (bytes 24-35): 2000-01-01 00:00:00 (fixo,
    # deterministico - nao usar datetime.now()).
    struct.pack_into(">HHHHHH", header, 24, 2000, 1, 1, 0, 0, 0)
    header[36:40] = b"acsp"  # file signature (obrigatorio)
    # primary platform (40-43), flags (44-47), manufacturer (48-51),
    # model (52-55), attributes (56-63) - todos zero
    # rendering intent (64-67) = 0 (perceptual)
    # illuminant (68-79) = D50 white point
    struct.pack_into(
        ">iii", header, 68,
        s15f16(WHITE_POINT[0]), s15f16(WHITE_POINT[1]), s15f16(WHITE_POINT[2]),
    )
    # creator (80-83), profile ID (84-99), reserved (100-127) - zero

    # Tag table.
    tag_table = struct.pack(">I", len(tags))
    for sig, offset, size in tag_entries:
        tag_table += struct.pack(">4sII", sig, offset, size)

    return bytes(header) + tag_table + b"".join(tag_blobs)


def main() -> int:
    if len(sys.argv) < 2:
        print(f"uso: {sys.argv[0]} <output.icc>", file=sys.stderr)
        return 2
    out = Path(sys.argv[1])
    out.parent.mkdir(parents=True, exist_ok=True)
    data = build_profile()
    out.write_bytes(data)
    # Header basico de sanity (sig 'acsp' no offset 36, RGB color space
    # no offset 16). Falha estruturada se o gerador errar a forma.
    if data[36:40] != b"acsp":
        print(f"ERRO: file signature esperada 'acsp', veio {data[36:40]!r}",
              file=sys.stderr)
        return 1
    if data[16:20] != b"RGB ":
        print(f"ERRO: color space esperado 'RGB ', veio {data[16:20]!r}",
              file=sys.stderr)
        return 1
    print(f"sRGB ICC v2 escrito em {out} ({len(data)} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
