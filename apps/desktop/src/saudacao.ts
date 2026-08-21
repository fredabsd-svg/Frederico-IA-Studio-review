/**
 * Saudação contextual da tela vazia.
 *
 * Função **pura** de propósito: recebe a hora, devolve o texto. A
 * hora vem de quem chama (`new Date().getHours()`), não daqui —
 * sem isso não há como conferir o texto das 3 da manhã sem esperar
 * as 3 da manhã.
 *
 * As faixas são as do handoff de design: 0–4h madrugada, 5–11h
 * manhã, 12–17h tarde, 18–23h noite.
 */

export interface Saudacao {
  /** Linha principal, 20px/600 na tela vazia. */
  titulo: string;
  /** Sublinha, 13.5px em `--fg-dim`. */
  sublinha: string;
}

/**
 * @param hora Hora local em 0–23. Valores fora da faixa caem na
 *   madrugada — é a única faixa em que "não sei que horas são" não
 *   produz uma saudação errada na cara do usuário ("Bom dia!" às
 *   onze da noite seria pior que genérico).
 */
export function saudacao(hora: number): Saudacao {
  if (!Number.isFinite(hora) || hora < 0 || hora > 23 || hora < 5) {
    return {
      titulo: "Ainda por aqui? Boa madrugada, coruja noturna 🦉",
      sublinha:
        "As melhores ideias não têm hora — o Studio acompanha o seu ritmo.",
    };
  }
  if (hora < 12) {
    return {
      titulo: "Bom dia! Café passado, sandbox de pé ☕",
      sublinha:
        "Comece pelo mais difícil — eu planejo as etapas e executo ao vivo.",
    };
  }
  if (hora < 18) {
    return {
      titulo: "Boa tarde! Hora de tirar planos do papel",
      sublinha:
        "Descreva a tarefa — eu planejo, executo e mostro tudo acontecendo.",
    };
  }
  return {
    titulo: "Boa noite, coruja noturna 🦉",
    sublinha: "Sessão noturna: foco total, luzes baixas, execução ao vivo.",
  };
}
