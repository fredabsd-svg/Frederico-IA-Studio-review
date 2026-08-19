# 0050 — Afirmação de fato em documento carrega a evidência junto

## Contexto

A Fase 8 encontrou **seis** casos de documento afirmando o que a medição não sustenta. Não são erros de digitação nem desatualização: em todos, alguém observou uma vez, concluiu, e escreveu a conclusão como fato medido.

| # | O documento afirmava | A medição mostrou | Onde ficou |
|---|---|---|---|
| 1 | o `CheckpointRepo` existe e deve ser estendido (ADR-0032 §D2) | `grep -rn "CheckpointRepo" --include=*.rs` não retorna nada | [ADR-0042](0042-projetos-e-checkpoints-nomeados.md) |
| 2 | o critério de saída do spike discrimina as bibliotecas (ADR-0040 §D2) | os dois candidatos passavam nele | [ADR-0047](0047-git-engine-usa-git2-medido-por-spike.md) §D3 |
| 3 | `PermissionSet::git` autoriza a categoria | é declaração; o `validate_tool_call` Passo 5 não lê permissão por categoria | `docs/modules/tool-registry.md` |
| 4 | o teste `project_path_stays_inside_jail` prova o invariante certo | contradizia o §D4 do próprio spec que o previa | `docs/modules/project-engine.md` §5 |
| 5 | o tema escuro passa em AA "em todos os pares testados" (ADR-0045) | mediu só contra `--bg`; `--erro` dava 4,18:1 sobre `--bg-elev` | `docs/architecture/ui-design-system.md` |
| 6 | alterar `.github/workflows/` impede o `ci.yml` de rodar na mesma PR | o PR #63, que originou a nota, teve CI verde em três runs alterando um workflow | errata no `status.md` |

O custo não é uniforme, e é isso que torna o padrão perigoso. O caso 1 custou um ADR inteiro de replanejamento. O caso 5 deixou uma falha de acessibilidade viva. O caso 6 custou uma PR desnecessária — barato, mas teria se repetido em toda sessão que lesse a nota.

**A forma é sempre a mesma:** uma observação vira conclusão, a conclusão vira frase afirmativa, e a frase perde o vínculo com o que a produziu. Quem lê depois não tem como distinguir "medido três vezes" de "aconteceu uma vez e eu supus".

A REGRA §1.6 já exige ADR para decisão. O que falta não é sobre decisão — é sobre **fato**.

## Decisões

### D1 — Afirmação de fato verificável carrega a evidência na mesma frase

Documento do projeto (ADR, spec, `status.md`, `CHANGELOG.md`, doc de módulo) que afirme um fato sobre o código, o CI ou o ambiente **cita junto o que o produziu**:

| Natureza do fato | Evidência que acompanha |
|---|---|
| "não existe no código" | o comando de busca, com o filtro (`grep -rn "X" --include=*.rs`) |
| "o CI faz / não faz" | número do run |
| "o teste prova" | nome do teste |
| "a medida é N" | como foi medida, ou o script que mede |
| "a biblioteca não suporta" | versão, e o erro ou a ausência na API |

Não é exigência de rigor acadêmico: é uma frase a mais, e ela transforma "não dá para saber se isso ainda vale" em "dá para conferir em dez segundos".

### D2 — Sem evidência, a frase muda de modo

Nem todo fato é medível na hora, e o remédio não é inventar medição. O remédio é **escrever no modo certo**:

- Medido: "o `CheckpointRepo` não existe — `grep -rn "CheckpointRepo" --include=*.rs` não retorna nada (2026-08-16)".
- Não medido: "**presumo** que o `CheckpointRepo` exista, herdado do ADR-0032; **não conferi**".

A segunda forma é honesta e serve de convite: quem passar por ali sabe que há o que verificar. A frase que este ADR combate não é a hipótese — é a hipótese **vestida de fato**.

### D3 — Uma observação não sustenta uma proibição

Os casos 2, 5 e 6 têm em comum a generalização a partir de um caso. Regra dura:

**Afirmação da forma "X sempre / X nunca / X impede" exige ou duas observações independentes, ou um mecanismo explicado.** Uma observação sustenta "aconteceu", não "acontece".

O caso 6 é o exemplo limpo: uma PR não rodou CI, e a conclusão registrada foi que *alterar workflow impede a CI*. Não havia mecanismo — e a segunda observação, quando veio, desmentiu.

### D4 — A régua vale para este ADR

Os seis casos da tabela do §Contexto carregam, cada um, o comando ou o número que os sustenta. O caso 6 cita os três runs; o caso 1, o `grep`; o caso 5, o par de tokens e a razão medida.

Um ADR sobre evidência escrito sem evidência seria o sétimo caso.

### D5 — Sem gate automático, e o motivo é declarado

Não há script que verifique isto, e não haverá nesta etapa. Um gate teria de decidir o que é "afirmação de fato" em prosa livre — problema de linguagem natural, não de varredura —, e um gate que erra em documentação produz a pior reação possível: quem escreve aprende a contornar a régua em vez de segui-la.

Isto é **regra de revisão**, e a fronteira é a mesma que o [ADR-0045](0045-fase-8-etapa-5b-identidade-visual-acessibilidade-e-sugestoes.md) §D2 traçou entre porta mecânica e avaliação humana: o que dá para medir vira gate; o que não dá, vira critério de revisão declarado. Fingir que a segunda é a primeira é o defeito, não a ausência de gate.

## Alternativas descartadas

1. **Gate por varredura** — recusar documento com "não existe" sem `grep` ao lado. Rejeitado pelo §D5: falso positivo em prosa livre ensina a contornar.
2. **Exigir evidência para toda frase.** Rejeitado por ruído: "o crate é puro" não precisa de citação; "o crate não importa `tauri`, verificado pelo `check-core-purity.ps1`" precisa — e a diferença é ser uma afirmação **verificável e contestável**.
3. **Registrar isto como nota na REGRA §1.6** em vez de ADR próprio. Rejeitado: §1.6 é sobre decisão precedendo código, e este é sobre fato precedendo afirmação. Colar num item existente esconderia a regra dentro de outro assunto — que é como a acessibilidade sumiu do roadmap (ADR-0045 §Contexto).
4. **Corrigir os seis casos e seguir.** Rejeitado: os seis já foram corrigidos, um a um, à medida que apareceram. Seis numa fase é padrão, e corrigir sintoma não muda a taxa.

## Consequências

- **Fica mais fácil:** confiar no documento. Uma afirmação com `grep` ao lado é conferível por quem duvida.
- **Fica mais difícil:** escrever rápido. É o efeito pretendido — a frase que custa dez segundos a mais é a que evita o replanejamento de uma etapa.
- **Sem gate**, isto depende de revisão. Declarado no §D5 em vez de disfarçado.
- **Os documentos existentes não são varridos retroativamente.** Reescrever tudo produziria centenas de citações inventadas depois do fato, que é o oposto do que este ADR quer. A régua vale para escrita nova e para o trecho que alguém tocar.

## Histórico de revisão

- 2026-08-19 — versão inicial, a partir dos seis casos da Fase 8.
