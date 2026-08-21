//! Fusão do catálogo embutido com o que o provedor respondeu.
//!
//! [ADR-0052] §D2 e §D3, em uma frase: **o remoto decide quais
//! modelos existem; o embutido decide quanto custam.**
//!
//! A assimetria não é estética. Medido em 2026-08-19: o `/models` do
//! OpenRouter devolve preço e janela de contexto; o da OpenAI devolve
//! só a lista de ids. Se o remoto mandasse em tudo, um refresh da
//! OpenAI apagaria todos os preços e o app pararia de rodar qualquer
//! modelo dela — `model_no_price` aborta o run antes de qualquer I/O
//! (`chat-and-providers.md` §462).
//!
//! [ADR-0052]: ../../docs/decisions/0052-refresh-de-catalogo-no-boot-em-segundo-plano.md

use frederico_core::{ModelId, ProviderId};

use crate::{Catalog, ModelDescriptor, PriceTable};

/// De onde veio o modelo que a UI está mostrando ([ADR-0043] §D4).
///
/// [ADR-0043]: ../../docs/decisions/0043-catalogo-embutido-com-refresh-opcional.md
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origem {
    /// Do catálogo embutido, sem confirmação do provedor nesta sessão.
    Embutido,
    /// Confirmado pelo provedor e conhecido pelo embutido — a
    /// combinação mais confiável: existe **e** tem preço.
    EmbutidoConfirmado,
    /// Só o provedor conhece. Pode não ter preço, e nesse caso não
    /// roda até o usuário informar (ADR-0043 §D3).
    Remoto,
}

/// Um modelo pronto para a UI, com a origem à vista.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeloEfetivo {
    pub descritor: ModelDescriptor,
    pub origem: Origem,
    /// `true` quando não há preço conhecido. A UI mostra e o run
    /// recusa — melhor que um custo silenciosamente errado.
    pub sem_preco: bool,
}

/// O que um provedor respondeu, já normalizado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RespostaDoProvedor {
    pub provider: ProviderId,
    pub modelos: Vec<ModeloRemotoNormalizado>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeloRemotoNormalizado {
    pub id: String,
    pub nome: Option<String>,
    pub janela_de_contexto: Option<u32>,
    pub entrada: Option<u64>,
    pub saida: Option<u64>,
}

/// Funde o embutido com as respostas que chegaram.
///
/// Regras, todas do [ADR-0052] §D2:
///
/// - Provedor **que não respondeu** fica exatamente como está. O
///   silêncio de um provedor nunca mexe na lista de outro.
/// - Modelo no remoto e não no embutido: entra, marcado.
/// - Modelo no embutido e não no remoto, **para um provedor que
///   respondeu**: sai. É modelo aposentado, e mantê-lo só produz
///   erro do provedor na hora do uso.
/// - Modelo nos dois: fica, com os campos do embutido preservados.
///
/// [ADR-0052]: ../../docs/decisions/0052-refresh-de-catalogo-no-boot-em-segundo-plano.md
#[must_use]
pub fn fundir(embutido: &Catalog, respostas: &[RespostaDoProvedor]) -> Vec<ModeloEfetivo> {
    let mut saida = Vec::new();

    for m in embutido.models() {
        let resposta = respostas.iter().find(|r| r.provider == m.provider);
        match resposta {
            // Provedor não consultado (ou que falhou): intacto.
            None => saida.push(ModeloEfetivo {
                descritor: m.clone(),
                origem: Origem::Embutido,
                sem_preco: sem_preco(&m.pricing_per_million),
            }),
            Some(r) => {
                if r.modelos.iter().any(|rm| rm.id == m.model.as_str()) {
                    saida.push(ModeloEfetivo {
                        descritor: m.clone(),
                        origem: Origem::EmbutidoConfirmado,
                        sem_preco: sem_preco(&m.pricing_per_million),
                    });
                }
                // Ausente do remoto: aposentado, não entra.
            }
        }
    }

    // Modelos que só o provedor conhece.
    for r in respostas {
        for rm in &r.modelos {
            let ja_tem = embutido
                .models()
                .iter()
                .any(|m| m.provider == r.provider && m.model.as_str() == rm.id);
            if ja_tem {
                continue;
            }
            let preco = PriceTable {
                input_microcents: rm.entrada.unwrap_or(0),
                output_microcents: rm.saida.unwrap_or(0),
            };
            // Sem preço informado **e** desconhecido do embutido: o
            // modelo aparece, mas não roda (ADR-0043 §D3). Zero aqui
            // não significa "de graça" — significa "não sabemos", e
            // é o `sem_preco` que carrega essa distinção.
            let desconhecido = rm.entrada.is_none() && rm.saida.is_none();
            saida.push(ModeloEfetivo {
                descritor: ModelDescriptor {
                    provider: r.provider.clone(),
                    model: ModelId::new(rm.id.clone()),
                    display_name: rm.nome.clone().unwrap_or_else(|| rm.id.clone()),
                    // Sem janela informada, assume o mínimo seguro em
                    // vez de um número inventado: quem consome trunca
                    // menos do que deveria, nunca mais.
                    context_window: rm.janela_de_contexto.unwrap_or(8_192),
                    modalities: crate::ModalitySet {
                        input: vec![crate::Modality::Text],
                        output: vec![crate::Modality::Text],
                    },
                    // **Nenhuma capacidade é presumida.** Declarar
                    // `tools` num modelo que não as suporta faz o run
                    // falhar no meio, depois de gastar tokens.
                    capabilities: crate::CapabilitySet::default(),
                    pricing_per_million: preco,
                },
                origem: Origem::Remoto,
                sem_preco: desconhecido || sem_preco(&preco),
            });
        }
    }

    saida.sort_by(|a, b| {
        a.descritor
            .provider
            .as_str()
            .cmp(b.descritor.provider.as_str())
            .then_with(|| a.descritor.model.as_str().cmp(b.descritor.model.as_str()))
    });
    saida
}

/// Preço zero em **ambos** os lados significa "não sabemos".
///
/// Modelo local (Ollama, LM Studio) também tem zero, e de propósito —
/// ele não custa mesmo. A diferença é que o embutido o declara assim,
/// então ele chega aqui como [`Origem::Embutido`] e a UI pode
/// distinguir os dois casos pela origem.
fn sem_preco(p: &PriceTable) -> bool {
    p.input_microcents == 0 && p.output_microcents == 0
}
