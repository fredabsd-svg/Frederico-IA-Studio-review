import { useEffect, useState } from "react";
import { getAppVersion } from "../services";
import { FASES } from "../generated/phase-status";

/**
 * Tela `/sobre`.
 *
 * **Nada aqui é escrito à mão sobre o estado do produto.** Duas
 * regras moldam este arquivo:
 *
 * 1. **Versão** — vem de `getAppVersion()`, que lê o binário em
 *    runtime. A fonte única é `tauri.conf.json`. O
 *    `check-docs.mjs` falha se um literal de versão aparecer no
 *    código do frontend (REGRAS §1.9).
 * 2. **Estado das fases** — vem de `generated/phase-status.ts`,
 *    derivado do `docs/status.md` pelo
 *    `scripts/generate-phase-status.mjs`. O CI falha se o
 *    gerado divergir da fonte (§1.10).
 *
 * A versão anterior desta tela mantinha as duas coisas à mão, e
 * as duas apodreceram: anunciava a versão errada e uma fase em
 * andamento que já tinha fechado havia cinco fases, listava tool
 * calls, memória e documentos como "não funciona ainda" com as
 * três fases concluídas, e chamava a `WindowsCredentialStore` de
 * stub depois de o DPAPI real ter entrado. Uma tela que promete o
 * que o código não faz é o defeito que a §1.1 existe para
 * prevenir — e esta era a mais visível de todas, porque é o
 * usuário que a lê.
 *
 * Por isso a lista de funcionalidades saiu daqui inteira. Ela não
 * é derivável do `status.md` sem prosa ambígua, e prosa mantida à
 * mão foi exatamente o que falhou. O que fica é o que a máquina
 * consegue manter honesto: versão, tabela de fases, e o ponteiro
 * para a fonte.
 */
export function About() {
  const [versao, setVersao] = useState<string | null>(null);
  const [erro, setErro] = useState<string | null>(null);

  useEffect(() => {
    let cancelado = false;
    getAppVersion()
      .then((v) => {
        if (!cancelado) setVersao(v);
      })
      .catch((e: unknown) => {
        if (!cancelado) setErro(e instanceof Error ? e.message : String(e));
      });
    return () => {
      cancelado = true;
    };
  }, []);

  const concluidas = FASES.filter((f) => f.estado === "concluída").length;

  return (
    <section>
      <h2>Sobre</h2>
      <p>
        Frederico IA Studio é um estúdio de IA desktop para Windows 10/11.{" "}
        {erro !== null ? (
          <span>Versão indisponível ({erro}).</span>
        ) : versao === null ? (
          <span>Carregando versão…</span>
        ) : (
          <span>
            Versão <strong>{versao}</strong>.
          </span>
        )}
      </p>

      <p>
        Estado por fase — {concluidas} de {FASES.length} concluídas. Derivado de{" "}
        <code>docs/status.md</code>, não escrito à mão:
      </p>
      <table>
        <thead>
          <tr>
            <th scope="col">Fase</th>
            <th scope="col">Nome</th>
            <th scope="col">Estado</th>
          </tr>
        </thead>
        <tbody>
          {FASES.map((f) => (
            <tr key={f.id}>
              <td>{f.id}</td>
              <td>{f.nome}</td>
              <td>{f.estado}</td>
            </tr>
          ))}
        </tbody>
      </table>

      <p>
        O projeto anterior morreu em parte porque a documentação prometia e o
        código divergia. Aqui a regra é a oposta: <code>docs/status.md</code> é
        a fonte da verdade do que está pronto, e uma fase só é marcada como
        concluída quando os testes dela passam. Esta tela lê essa fonte em vez
        de repeti-la — o detalhe de cada fase, incluindo as pendências
        conhecidas, está lá.
      </p>
    </section>
  );
}
