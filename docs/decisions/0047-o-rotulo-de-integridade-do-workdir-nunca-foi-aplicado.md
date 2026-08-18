# 0047 — O rótulo de integridade do workdir nunca foi aplicado: o sandbox bloqueava tudo, não a fuga

## Contexto

A Etapa 5+ da Fase 7 declarou path safety fechada com duas peças combinadas: o filho nasce com `TokenIntegrityLevel = Low` e o workdir recebe `Mandatory Label\Low` via `set_low_integrity_label`. A ideia é a do próprio ADR-0031: o filho escreve **dentro** do workdir (rótulos iguais) e é barrado **fora** dele (objeto Medium, política `NO_WRITE_UP`).

A segunda peça nunca funcionou.

O achado apareceu de lado, durante a investigação do ADR-0046: um teste tentou gravar um arquivo de ticks dentro do próprio workdir para medir o relógio do filho, e levou `PermissionError: [Errno 13] Permission denied: 'tick.txt'`.

### O que foi medido (2026-08-18, Windows 11 26200)

**1. O rótulo não estava no workdir.** O `icacls` no workdir não mostrava linha de rótulo alguma, nem antes nem depois de `set_low_integrity_label` — que retornava `Ok` e imprimia `[spawn-debug] Step 1 OK`. Um rótulo de verdade aparece assim (referência obtida com `icacls <dir> /setintegritylevel L`):

```text
Rótulo Obrigatório\Nível Obrigatório Baixo:(NW)
```

**2. A causa está na montagem do descritor de segurança.** `build_low_label_security_descriptor` declara `SE_SELF_RELATIVE` mas escreve o SACL **através da struct `SECURITY_DESCRIPTOR` do crate `windows`**, que modela a forma **absoluta**: 40 bytes, com `Owner`/`Group`/`Sacl`/`Dacl` como ponteiros de 8 bytes — `.Sacl` no offset 24. Na forma self-relative esses quatro campos são **offsets `u32` nos bytes 4/8/12/16**.

Resultado: o ponteiro ia para o byte 24, dentro da área de dados que a cópia do ACL sobrescrevia logo em seguida, e o byte 12 (`OffsetSacl`) nunca era escrito. Dump do header:

```text
header: [01, 00, 10, 80, 00 00 00 00, 00 00 00 00, 00 00 00 00, 00 00 00 00]
control=0x8010 (SE_SELF_RELATIVE|SE_SACL_PRESENT)
offsets: owner=0 group=0 sacl=0 dacl=0
IsValidSecurityDescriptor = true
GetSecurityDescriptorSacl -> presente=true  sacl_null=true
```

`SE_SACL_PRESENT` ligado com offset zero é **"SACL presente porém NULL"** — um descritor *válido* que significa "nenhum rótulo". Por isso o `SetFileSecurityW` devolvia sucesso: ele aplicava fielmente um pedido vazio. O `Step 1 OK` reportava o êxito de um no-op.

**3. A consequência foi negação total de escrita, não path safety.** Com o workdir em integridade Medium (o padrão) e o filho em Low, a política `NO_WRITE_UP` barrava tudo:

```text
CWD: ...\.tmpDMM0Lo
criar arquivo no CWD: FALHOU errno=13 winerror=5
criar subdir no CWD:  FALHOU errno=13 winerror=5
listar CWD: OK                      <- leitura passa (read-up é permitido)
criar no parent: FALHOU
criar em %TEMP% absoluto: FALHOU
```

`exec.python`, `exec.node` e `exec.shell` não conseguiam produzir **nenhum** arquivo, nem na pasta que receberam para trabalhar.

**4. O teste de path safety passava por um motivo mais fraco que o nome dele.** O `child_cannot_write_outside_workspace` afirma provar que o filho não escapa do jail. Como o filho não escrevia em lugar nenhum, o teste não distinguia "não escapou" de "não escreve nada" — e ficava verde.

**5. Dois comentários no `jail.rs` afirmavam o contrário do código.** O bloco do Step 3 dizia que "o child ainda pode escrever no workdir (que tem o label Low aplicado via `SetFileSecurityW` no step 1)" — nunca teve — e que "o test do `print()` e do wall-clock falham com IOError porque o `print` é bloqueado". Não falham: o stdout do filho chega normalmente, porque o check obrigatório do pipe acontece na **abertura** do handle, feita pelo processo pai, e não a cada escrita.

## Decisão

**Montar o descritor self-relative pelo formato dele, não pela struct da forma absoluta.** O header dos 20 bytes passa a ser escrito byte a byte: `Revision`, `Sbz1`, `Control` (`u16`) e os quatro offsets `u32`, com `OffsetSacl = 20`. O tamanho do buffer passa a vir do `AclSize` real em vez dos 256 bytes fixos.

A struct `SECURITY_DESCRIPTOR` do crate `windows` sai da função. Ela não é o tipo errado por acidente: ela é a forma **absoluta**, e a self-relative não tem struct correspondente no crate. Usar uma para escrever a outra é o defeito, e o comentário no código agora diz isso, para que a "simplificação" não volte.

**O teste de path safety ganha controle positivo obrigatório.** O `child_cannot_write_outside_workspace` passa a fazer as duas metades na mesma invocação: grava `dentro.txt` no workdir (tem de funcionar) e tenta `..\evil.txt` (tem de falhar). Sem o par, o teste não distingue "não escapou" de "não escreve nada" — que é exatamente como ele viveu verde até aqui.

## Alternativas descartadas

**`SetNamedSecurityInfoW` / `SetSecurityInfo` em vez de `SetFileSecurityW`.** A API não era o problema — o `SetFileSecurityW` fazia o que lhe pediram. Trocar a API teria escondido o defeito atrás de uma função diferente, possivelmente com o mesmo descritor quebrado, e o histórico do arquivo mostra que essa troca já tinha sido feita uma vez por diagnóstico errado.

**`ConvertStringSecurityDescriptorToSecurityDescriptorW("S:(ML;;NW;;;LW)")`.** Constrói o descritor correto e é menos código. Descartada porque devolve memória do `LocalAlloc`, que precisa de `LocalFree` e de um wrapper de lifetime próprio, e porque esconde num literal SDDL a única coisa que este ADR precisa deixar explícita: a diferença entre a forma absoluta e a self-relative. Fica registrada como opção legítima caso o header manual venha a incomodar.

**Deixar como está e documentar "o filho não escreve".** É o comportamento que existia de fato, e teria a virtude de ser verdadeiro. Descartada porque contraria o ADR-0031 e torna as três ferramentas de execução incapazes de entregar resultado — uma ferramenta que só lê não é a ferramenta que a Fase 7 especificou.

## Consequências

**Fica mais fácil.** O rótulo passa a aparecer no `icacls`, e o sandbox bloqueia a **fuga** em vez de bloquear tudo:

```text
criar arquivo no CWD: OK        criar no parent: FALHOU
criar subdir no CWD:  OK        criar em %TEMP% absoluto: FALHOU
```

`exec.python`/`exec.node`/`exec.shell` voltam a poder produzir arquivo no workspace da conversa, que é o que o produto promete. E o teste de path safety passa a provar o que o nome dele diz.

**Fica mais difícil.** Isto **relaxa** o comportamento que existia na prática: até aqui o filho não escrevia em lugar nenhum, o que era mais restritivo do que o desenho. Quem lesse só o comportamento observado poderia tomar essa restrição acidental como garantia. Ela nunca foi garantia — era um bug com aparência de proteção —, mas a mudança é real e está registrada aqui para não ser descoberta de novo por acidente.

**O que este ADR não muda.** As três lacunas nomeadas no `SECURITY.md` continuam de pé: **read-up** (o rótulo só bloqueia escrita para cima; o filho lê caminhos Medium), **rede** (`getaddrinfo` direto não passa pela allowlist) e **pipe labels** (`CreatePipe` com SACL exige `SeSecurityPrivilege`). A terceira ganha uma correção de fato: os pipes seguem sem rótulo, mas isso **não** impede o filho de escrever neles, ao contrário do que o comentário do `jail.rs` afirmava.

**Sobre a janela em que isto valeu.** O defeito entrou com a Etapa 5+ (2026-08-10) e viveu até 2026-08-18, atravessando a promoção da Fase 7 a `concluída` e as duas reaberturas dela. Nenhum teste o pegou porque o único teste que olhava para esse comportamento estava formulado de um jeito que o defeito satisfazia. É o mesmo padrão do ADR-0046: o mecanismo que nunca é exercitado no caminho real parece funcionar até o dia em que precisa.
