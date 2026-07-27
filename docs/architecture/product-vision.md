<!--
Estado: especificado
Verificado contra o código em: —
Fase correspondente: 1-9 (global)
-->

# Visão do Produto

Aplicativo desktop de IA para Windows 10/11, distribuído como instalador `.exe`, que permite conversar com múltiplos provedores/modelos, usar ferramentas reais, gerar documentos profissionais (Word/Excel/PDF) e desenvolver software com auditoria completa do que a IA fez.

Este documento é a **âncora** do repositório. Princípios e não-objetivos aqui declarados valem para todas as fases; decisões específicas de implementação vivem em `docs/decisions/`.

## Princípios inegociáveis

Referência: `PROMPT MESTRE` §4.

- **Transparência** — o sistema nunca finge o que não fez (não declara arquivo criado, ferramenta executada ou teste passado sem prova real).
- **Estado real** — a UI reflete exatamente o que aconteceu, incluindo falhas, pausas, truncamentos e etapas pendentes.
- **Persistência** — nada crítico vive apenas em RAM. Execuções, mensagens, memórias e artefatos sobrevivem a fechamento, queda e reinicialização.
- **Segurança por padrão** — ferramentas perigosas nascem bloqueadas; ativação é decisão consciente do usuário, auditada.
- **Qualidade documental** — Word, Excel e PDF são cidadãos de primeira classe, com identidade visual própria e validação bloqueante.
- **Zero suposição silenciosa** — "não sei", "não confirmado", "indisponível" são respostas válidas. A UI nunca esconde incerteza.

## Não-objetivos

Decididos na fundação, valem até decisão em contrário via ADR:

- **Versão servidor/web** na v1. O núcleo é desacoplado (ver ADR-0003) para permitir isso no futuro, mas a casca servidor não é construída agora.
- **Multi-usuário, multi-tenant, autenticação federada.** A v1 é um aplicativo de usuário único por máquina.
- **Mobile (Android, iOS), macOS, Linux.** Apenas Windows 10/11 64 bits.
- **Docker, WSL, PostgreSQL, Node.js, Python, Redis, banco vetorial, compilador, servidor web como dependência que o usuário precise instalar manualmente** (`PROMPT MESTRE` §5.2).
- **Empacotar Office, navegador ou qualquer software de terceiros** que o usuário não precise ter. Tudo embutido ou desnecessário.
- **Compatibilidade com banco, código ou API do projeto anterior** (`PROMPT MESTRE` §2 — proibição expressa). O Frederico IA Studio nasce do zero.
- **Emulação textual de tool calling como caminho padrão.** Pode existir como modo experimental, claramente identificado e coberto por testes (`PROMPT MESTRE` §7.5), nunca como fallback silencioso.
- **Plugins de terceiros além de provedores compatíveis com a API OpenAI** e ferramentas do catálogo do `PROMPT MESTRE` §7.11.

## Decisões

Nenhuma decisão estrutural nova. Este documento é a âncora; decisões específicas vivem em `docs/decisions/`. As decisões tomadas na fundação que este documento ancora são:

- [ADR-0001](../decisions/0001-spec-vs-as-built.md) — como coexistir com `REGRAS §1.1`.
- [ADR-0002](../decisions/0002-monorepo-layout.md) — layout do repositório.
- [ADR-0003](../decisions/0003-nucleo-desacoplado-da-casca-tauri.md) — separação núcleo/casca.
- [ADR-0004](../decisions/0004-document-worker-em-python-embutido.md) — tecnologia do `document-worker`.

## Referências

- `REGRAS-DO-PROJETO.md` §1.1 (princípio da documentação honesta)
- [`development-roadmap.md`](./development-roadmap.md) — o que entra em cada fase
- [`software-architecture.md`](./software-architecture.md) — como o produto é montado
- `PROMPT MESTRE` §3 (visão), §4 (princípios), §5 (plataforma), §33 (primeira ação)
