//! Liga o cliente de `lsp.rs` aos providers do editor do gpui-base.

use std::{
    ops::Range,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicI32, Ordering},
    },
};

use futures::channel::oneshot;
use gpui_kit::{
    base::input::{
        CodeActionProvider, CompletionProvider, DefinitionProvider, HoverProvider, Rope, RopeExt,
    },
    component::input::EditorState,
    *,
};
use lsp_types::{
    CodeAction, CodeActionOrCommand, CompletionContext, CompletionResponse, CompletionTextEdit,
    GotoDefinitionResponse, Hover, LocationLink, TextEdit,
};
use serde_json::{Value, json};

use crate::lsp::{self, Client};

/// Um arquivo aberto no servidor: o texto que ele conhece e a versão.
pub struct Doc {
    pub client: Arc<Client>,
    pub path: PathBuf,
    pub uri: String,
    version: AtomicI32,
    last: Mutex<String>,
}

impl Doc {
    pub fn open(client: Arc<Client>, path: PathBuf, text: String) -> Arc<Self> {
        client.did_open(&path, &text);
        Arc::new(Self {
            uri: lsp::file_uri(&path),
            client,
            path,
            version: AtomicI32::new(1),
            last: Mutex::new(text),
        })
    }

    /// Manda o texto novo se mudou desde o último envio (os providers chamam antes de cada pedido).
    pub fn sync(&self, text: &str) {
        let mut last = self.last.lock().unwrap();
        if *last == text {
            return;
        }
        let v = self.version.fetch_add(1, Ordering::Relaxed) + 1;
        self.client.did_change(&self.path, v, text);
        *last = text.to_string();
    }

    pub fn diagnostics(&self) -> Vec<lsp_types::Diagnostic> {
        self.client.diagnostics(&self.uri)
    }

    fn at(&self, text: &Rope, offset: usize) -> Value {
        self.sync(&text.to_string());
        json!({"textDocument": {"uri": self.uri}, "position": text.offset_to_position(offset)})
    }
}

impl Drop for Doc {
    fn drop(&mut self) {
        self.client.did_close(&self.path);
    }
}

async fn answer(rx: oneshot::Receiver<Result<Value, String>>) -> Result<Value> {
    let r = rx
        .await
        .map_err(|_| std::io::Error::other("o servidor de linguagem fechou"))?;
    Ok(r.map_err(std::io::Error::other)?)
}

pub struct Provider(pub Arc<Doc>);

/// O menu do gpui-base não filtra: fica o que tem as letras digitadas em ordem, quem começa com elas primeiro.
pub fn filter(
    items: Vec<lsp_types::CompletionItem>,
    typed: &str,
) -> Vec<lsp_types::CompletionItem> {
    let typed = typed.to_lowercase();
    let key = |i: &lsp_types::CompletionItem| {
        i.filter_text
            .clone()
            .unwrap_or_else(|| i.label.clone())
            .to_lowercase()
    };
    let has = |k: &str| {
        let mut cs = k.chars();
        typed.chars().all(|t| cs.any(|c| c == t))
    };
    let mut out: Vec<_> = items.into_iter().filter(|i| has(&key(i))).collect();
    out.sort_by_key(|i| {
        (
            !key(i).starts_with(&typed),
            i.sort_text.clone().unwrap_or_else(|| i.label.clone()),
        )
    });
    out
}

/// Início do identificador que está sendo digitado: o item do completar substitui daí até o cursor.
/// Sem isso o menu do gpui-base troca desde onde abriu (`nome.` virava `toString`).
pub fn word_start(text: &Rope, offset: usize) -> usize {
    let s = text.to_string();
    let before = s.get(..offset).unwrap_or_default();
    before
        .char_indices()
        .rev()
        .take_while(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '$'))
        .last()
        .map_or(offset, |(i, _)| i)
}

/// O gpui-base manda a palavra inteira como `triggerCharacter` (o TS 7 recusa): só `.`/`:` antes do cursor
/// contam como caractere de disparo; o resto é pedido normal.
fn context(text: &Rope, offset: usize, trigger: CompletionContext) -> Value {
    let before = text
        .to_string()
        .get(..offset)
        .and_then(|s| s.chars().last());
    match before {
        Some(c @ ('.' | ':')) => json!({"triggerKind": 2, "triggerCharacter": c.to_string()}),
        _ if trigger.trigger_kind
            == lsp_types::CompletionTriggerKind::TRIGGER_FOR_INCOMPLETE_COMPLETIONS =>
        {
            json!({"triggerKind": 3})
        }
        _ => json!({"triggerKind": 1}),
    }
}

impl CompletionProvider for Provider {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<CompletionResponse>> {
        let mut p = self.0.at(text, offset);
        p["context"] = context(text, offset, trigger);
        let rx = self.0.client.request("textDocument/completion", p);
        let start = word_start(text, offset);
        let typed = text.slice(start..offset).to_string();
        let word = lsp_types::Range::new(
            text.offset_to_position(start),
            text.offset_to_position(offset),
        );
        cx.background_spawn(async move {
            let v = answer(rx).await?;
            if v.is_null() {
                return Ok(CompletionResponse::Array(Vec::new()));
            }
            let mut items = match serde_json::from_value(v)? {
                CompletionResponse::Array(i) => i,
                CompletionResponse::List(l) => l.items,
            };
            for i in items.iter_mut().filter(|i| i.text_edit.is_none()) {
                let new_text = i.insert_text.clone().unwrap_or_else(|| i.label.clone());
                i.text_edit = Some(CompletionTextEdit::Edit(TextEdit::new(word, new_text)));
            }
            Ok(CompletionResponse::Array(filter(items, &typed)))
        })
    }

    fn is_completion_trigger(&self, _: usize, new_text: &str, _: &mut App) -> bool {
        new_text
            .chars()
            .last()
            .is_some_and(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | ':' | '$'))
    }
}

impl HoverProvider for Provider {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<Hover>>> {
        let rx = self
            .0
            .client
            .request("textDocument/hover", self.0.at(text, offset));
        cx.background_spawn(async move {
            let v = answer(rx).await?;
            Ok(serde_json::from_value(v).ok())
        })
    }
}

pub fn links(r: GotoDefinitionResponse) -> Vec<LocationLink> {
    let link = |l: lsp_types::Location| LocationLink {
        origin_selection_range: None,
        target_uri: l.uri,
        target_range: l.range,
        target_selection_range: l.range,
    };
    match r {
        GotoDefinitionResponse::Scalar(l) => vec![link(l)],
        GotoDefinitionResponse::Array(ls) => ls.into_iter().map(link).collect(),
        GotoDefinitionResponse::Link(ls) => ls,
    }
}

impl DefinitionProvider for Provider {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<LocationLink>>> {
        let rx = self
            .0
            .client
            .request("textDocument/definition", self.0.at(text, offset));
        cx.background_spawn(async move {
            let v = answer(rx).await?;
            Ok(serde_json::from_value(v).map(links).unwrap_or_default())
        })
    }
}

/// As edições da ação neste arquivo; ponytail: edição em outros arquivos fica de fora.
fn edits_here(action: &CodeAction, uri: &str) -> Vec<TextEdit> {
    let Some(edit) = &action.edit else {
        return Vec::new();
    };
    let mut out: Vec<TextEdit> = edit
        .changes
        .iter()
        .flatten()
        .filter(|(u, _)| u.as_str() == uri)
        .flat_map(|(_, e)| e.clone())
        .collect();
    if let Some(lsp_types::DocumentChanges::Edits(docs)) = &edit.document_changes {
        for d in docs.iter().filter(|d| d.text_document.uri.as_str() == uri) {
            out.extend(d.edits.iter().map(|e| match e {
                lsp_types::OneOf::Left(e) => e.clone(),
                lsp_types::OneOf::Right(a) => a.text_edit.clone(),
            }));
        }
    }
    // de trás para frente: aplicar uma não desloca as posições das outras
    out.sort_by(|a, b| {
        (b.range.start.line, b.range.start.character)
            .cmp(&(a.range.start.line, a.range.start.character))
    });
    out
}

impl CodeActionProvider for Provider {
    fn id(&self) -> SharedString {
        "lsp".into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: Range<usize>,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<CodeAction>>> {
        let text = Rope::from(state.read(cx).value().as_ref());
        self.0.sync(&text.to_string());
        let (start, end) = (
            text.offset_to_position(range.start),
            text.offset_to_position(range.end),
        );
        let diags: Vec<_> = self
            .0
            .diagnostics()
            .into_iter()
            .filter(|d| d.range.start.line <= end.line && d.range.end.line >= start.line)
            .collect();
        let rx = self.0.client.request(
            "textDocument/codeAction",
            json!({
                "textDocument": {"uri": self.0.uri},
                "range": {"start": start, "end": end},
                "context": {"diagnostics": diags},
            }),
        );
        cx.background_spawn(async move {
            let v = answer(rx).await?;
            let list: Vec<CodeActionOrCommand> = serde_json::from_value(v).unwrap_or_default();
            Ok(list
                .into_iter()
                .filter_map(|a| match a {
                    CodeActionOrCommand::CodeAction(a) => Some(a),
                    CodeActionOrCommand::Command(_) => None,
                })
                .collect())
        })
    }

    fn perform_code_action(
        &self,
        state: Entity<EditorState>,
        action: CodeAction,
        _: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let doc = self.0.clone();
        let resolve = (action.edit.is_none() && action.data.is_some()).then(|| {
            doc.client.request(
                "codeAction/resolve",
                serde_json::to_value(&action).unwrap_or_default(),
            )
        });
        window.spawn(cx, async move |cx| {
            let action = match resolve {
                Some(rx) => serde_json::from_value(answer(rx).await?).unwrap_or(action),
                None => action,
            };
            let edits = edits_here(&action, &doc.uri);
            cx.update(|window, cx| {
                state.update(cx, |s, cx| s.apply_lsp_edits(&edits, window, cx));
            })?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Rope, filter, json, links, word_start};

    #[test]
    fn filtra_pelo_digitado() {
        let item = |l: &str| lsp_types::CompletionItem {
            label: l.into(),
            ..Default::default()
        };
        let got: Vec<_> = filter(
            vec![
                item("charAt"),
                item("toLocaleUpperCase"),
                item("toUpperCase"),
            ],
            "toU",
        )
        .into_iter()
        .map(|i| i.label)
        .collect();
        assert_eq!(got, ["toUpperCase", "toLocaleUpperCase"]);
    }

    #[test]
    fn palavra_antes_do_cursor() {
        let t = Rope::from("const x = nome.toSt");
        assert_eq!(word_start(&t, t.len()), "const x = nome.".len());
        assert_eq!(
            word_start(&t, "const x = nome.".len()),
            "const x = nome.".len()
        );
        assert_eq!(word_start(&t, "const x = nom".len()), "const x = ".len());
    }

    #[test]
    fn definicao_vira_link() {
        let v = json!([{"uri": "file:///a.ts", "range": {"start": {"line": 3, "character": 1}, "end": {"line": 3, "character": 5}}}]);
        let l = links(serde_json::from_value(v).unwrap());
        assert_eq!(l[0].target_selection_range.start.line, 3);
        assert_eq!(l[0].target_uri.as_str(), "file:///a.ts");
    }
}
