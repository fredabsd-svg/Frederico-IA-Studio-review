//! Testes da fusão embutido × remoto — ADR-0052 §D2 e §D3.

use frederico_core::{ModelId, ProviderId};
use frederico_model_catalog::{
    fundir, Catalog, CatalogHandle, ModeloRemotoNormalizado, Origem, RespostaDoProvedor,
};

fn remoto(id: &str, entrada: Option<u64>, saida: Option<u64>) -> ModeloRemotoNormalizado {
    ModeloRemotoNormalizado {
        id: id.to_string(),
        nome: None,
        janela_de_contexto: None,
        entrada,
        saida,
    }
}

/// Sem resposta nenhuma, a fusão é o catálogo embutido inteiro.
#[test]
fn sem_resposta_o_catalogo_fica_intacto() {
    let cat = Catalog::load();
    let efetivo = fundir(cat, &[]);
    assert_eq!(efetivo.len(), cat.models().len());
    assert!(efetivo.iter().all(|m| m.origem == Origem::Embutido));
}

/// **O §D2 na prática:** modelo que o provedor não lista mais sai.
#[test]
fn modelo_aposentado_pelo_provedor_sai_da_lista() {
    let cat = Catalog::load();
    let anthropic = ProviderId::new("anthropic");
    let embutidos: Vec<String> = cat
        .list_for_provider(&anthropic)
        .iter()
        .map(|m| m.model.as_str().to_string())
        .collect();
    assert!(
        embutidos.len() >= 2,
        "o teste precisa de ao menos 2 modelos embutidos da anthropic"
    );

    // O provedor responde com só o primeiro.
    let resposta = RespostaDoProvedor {
        provider: anthropic.clone(),
        modelos: vec![remoto(&embutidos[0], None, None)],
    };
    let efetivo = fundir(cat, &[resposta]);

    let sobreviveram: Vec<&str> = efetivo
        .iter()
        .filter(|m| m.descritor.provider == anthropic)
        .map(|m| m.descritor.model.as_str())
        .collect();
    assert_eq!(
        sobreviveram,
        vec![embutidos[0].as_str()],
        "só o modelo que o provedor confirmou deveria ficar"
    );
}

/// **O §D3 na prática:** o preço continua vindo do embutido, mesmo
/// quando o provedor não informa preço nenhum.
#[test]
fn preco_do_embutido_sobrevive_a_resposta_sem_preco() {
    let cat = Catalog::load();
    let anthropic = ProviderId::new("anthropic");
    let primeiro = cat.list_for_provider(&anthropic)[0].clone();
    assert!(
        primeiro.pricing_per_million.input_microcents > 0,
        "o teste precisa de um modelo embutido com preço"
    );

    // Resposta no formato da OpenAI: só o id, sem preço.
    let resposta = RespostaDoProvedor {
        provider: anthropic.clone(),
        modelos: vec![remoto(primeiro.model.as_str(), None, None)],
    };
    let efetivo = fundir(cat, &[resposta]);
    let m = efetivo
        .iter()
        .find(|m| m.descritor.model == primeiro.model)
        .expect("o modelo confirmado tem que estar na fusão");

    assert_eq!(
        m.descritor.pricing_per_million, primeiro.pricing_per_million,
        "o preço do embutido não pode ser apagado por uma resposta sem preço — \
         seria `model_no_price` em todo modelo da OpenAI"
    );
    assert_eq!(m.origem, Origem::EmbutidoConfirmado);
    assert!(!m.sem_preco);
}

/// Modelo que só o provedor conhece entra, marcado como remoto.
#[test]
fn modelo_novo_do_provedor_entra_marcado() {
    let cat = Catalog::load();
    let resposta = RespostaDoProvedor {
        provider: ProviderId::new("openrouter"),
        modelos: vec![remoto(
            "fabricante/modelo-novissimo",
            Some(250_000),
            Some(1_500_000),
        )],
    };
    let efetivo = fundir(cat, &[resposta]);
    let novo = efetivo
        .iter()
        .find(|m| m.descritor.model.as_str() == "fabricante/modelo-novissimo")
        .expect("modelo novo tem que entrar");

    assert_eq!(novo.origem, Origem::Remoto);
    assert!(!novo.sem_preco);
    assert_eq!(novo.descritor.pricing_per_million.input_microcents, 250_000);
    // **Nenhuma capacidade presumida:** declarar `tools` num modelo
    // que não as suporta faz o run falhar no meio, depois de gastar
    // tokens.
    assert!(novo.descritor.capabilities.capabilities.is_empty());
}

/// **Negação:** modelo novo sem preço aparece, mas marcado — e é o
/// `sem_preco` que impede o custo silenciosamente errado.
#[test]
fn modelo_novo_sem_preco_e_marcado_em_vez_de_valer_zero() {
    let cat = Catalog::load();
    let resposta = RespostaDoProvedor {
        provider: ProviderId::new("openai"),
        modelos: vec![remoto("gpt-sem-preco", None, None)],
    };
    let efetivo = fundir(cat, &[resposta]);
    let novo = efetivo
        .iter()
        .find(|m| m.descritor.model.as_str() == "gpt-sem-preco")
        .expect("entra na lista");

    assert!(
        novo.sem_preco,
        "preço zero de um modelo pago é custo errado em silêncio, que é pior \
         que recusar o run"
    );
}

/// **Negação:** o silêncio de um provedor não mexe na lista de outro.
#[test]
fn resposta_de_um_provedor_nao_afeta_os_outros() {
    let cat = Catalog::load();
    // Comparação por conjunto: a fusão ordena por provedor e modelo
    // de propósito (ordem estável na UI), então comparar a ordem do
    // JSON com a da saída mediria a ordenação, não o isolamento.
    let mut antes: Vec<String> = cat
        .list_for_provider(&ProviderId::new("deepseek"))
        .iter()
        .map(|m| m.model.as_str().to_string())
        .collect();
    antes.sort();

    // Só a anthropic responde, e responde vazio.
    let resposta = RespostaDoProvedor {
        provider: ProviderId::new("anthropic"),
        modelos: vec![],
    };
    let efetivo = fundir(cat, &[resposta]);

    let mut depois: Vec<String> = efetivo
        .iter()
        .filter(|m| m.descritor.provider.as_str() == "deepseek")
        .map(|m| m.descritor.model.as_str().to_string())
        .collect();
    depois.sort();
    assert_eq!(antes, depois);

    // E a anthropic, que respondeu vazio, ficou sem modelo.
    assert!(!efetivo
        .iter()
        .any(|m| m.descritor.provider.as_str() == "anthropic"));
}

// --- O handle compartilhado -----------------------------------------
//
// Estes testes fecham o buraco que a primeira ligação deixou: a UI
// lia a lista fundida e o motor de execução validava contra a
// embutida. As duas discordavam sobre quais modelos existem, e
// escolher um modelo que só o remoto conhecia dava `ModelNotFound`
// no meio da conversa — erro do provedor, na cara do usuário, por
// um modelo que o próprio app tinha oferecido na lista suspensa.

/// O motor enxerga o que o refresh publicou, não o embutido.
///
/// Percorre o caminho de produção inteiro: `fundir` → `from_models`
/// → `replace` → `find_model`. É o percurso que estava cortado no
/// meio.
#[test]
fn o_que_o_refresh_publica_e_o_que_o_motor_le() {
    let handle = CatalogHandle::new(std::sync::Arc::new(Catalog::load().clone()));
    let deepseek = ProviderId::from("deepseek");
    let inedito = ModelId::from("modelo-que-so-o-remoto-conhece");

    assert!(
        handle.current().find_model(&deepseek, &inedito).is_none(),
        "pré-condição: o embutido não conhece este modelo"
    );

    let fundido = fundir(
        Catalog::load(),
        &[RespostaDoProvedor {
            provider: deepseek.clone(),
            modelos: vec![remoto(
                "modelo-que-so-o-remoto-conhece",
                Some(1000),
                Some(2000),
            )],
        }],
    );
    handle.replace(std::sync::Arc::new(Catalog::from_models(
        fundido.into_iter().map(|m| m.descritor).collect(),
    )));

    assert!(
        handle.current().find_model(&deepseek, &inedito).is_some(),
        "depois do refresh, o motor tem de resolver o modelo que a UI oferece"
    );
}

/// Um `Arc` obtido antes da troca continua coerente: a publicação
/// substitui o ponteiro inteiro, não muta a lista sob os pés de um
/// run em andamento.
#[test]
fn a_lista_de_um_run_em_andamento_nao_muda_sob_os_pes_dele() {
    let handle = CatalogHandle::new(std::sync::Arc::new(Catalog::load().clone()));
    let antes = handle.current();
    let total_antes = antes.list_all().len();
    assert!(total_antes > 0, "pré-condição: o embutido não é vazio");

    handle.replace(std::sync::Arc::new(Catalog::from_models(vec![])));

    assert_eq!(
        antes.list_all().len(),
        total_antes,
        "o snapshot que o run pegou não pode encolher no meio do caminho"
    );
    assert_eq!(
        handle.current().list_all().len(),
        0,
        "quem pedir depois vê a lista nova"
    );
}
