//! O que o GitHub tem de diferente, traduzido para o que o resto do Maestro já fala.
//!
//! A tela, o revisor e o diff foram escritos contra as formas do GitLab (discussão com `notes`,
//! `position`, `approvals`). Em vez de reescrever tudo, as respostas do GitHub entram aqui e saem
//! nessas mesmas formas: o PR vira MR, o review comment vira discussão, o review vira aprovação.
//! O caminho inverso — o que o Maestro escreve — sai daqui já no formato do GitHub.

use serde_json::{Value, json};

use crate::{forja::Forja, mrs::Row};

pub fn base(path: &str, n: u64) -> String {
    format!("repos/{path}/pulls/{n}")
}

/// A fila: o GitHub responde a busca em formato de issue, sem branch nem sha.
pub fn rows(busca: &Value) -> Vec<Row> {
    let itens = busca["items"].as_array().or(busca.as_array());
    itens
        .into_iter()
        .flatten()
        .map(|m| Row {
            forja: Forja::GitHub,
            // `https://api.github.com/repos/dono/repo` → `dono/repo`
            path: m["repository_url"]
                .as_str()
                .unwrap_or_default()
                .rsplit("/repos/")
                .next()
                .unwrap_or_default()
                .into(),
            iid: m["number"].as_u64().unwrap_or_default(),
            title: m["title"].as_str().unwrap_or_default().into(),
            who: format!("@{}", m["user"]["login"].as_str().unwrap_or_default()),
            sha: String::new(),
            source: String::new(),
            target: String::new(),
            web_url: m["html_url"].as_str().unwrap_or_default().into(),
            ci: None,
            notes: m["comments"].as_u64().unwrap_or_default(),
            updated: m["updated_at"].as_str().unwrap_or_default().into(),
        })
        .collect()
}

/// O detalhe do PR completa o que a busca não traz.
pub fn completa(r: &mut Row, detail: &Value) {
    r.sha = detail["head"]["sha"].as_str().unwrap_or_default().into();
    r.source = detail["head"]["ref"].as_str().unwrap_or_default().into();
    r.target = detail["base"]["ref"].as_str().unwrap_or_default().into();
}

/// Os arquivos do PR no formato do `/diffs` do GitLab: (novo, velho, corpo do diff) e o patch colado.
pub fn arquivos(files: &Value) -> (Vec<(String, String, String)>, String) {
    let mut raw = Vec::new();
    let mut patch = String::new();
    for f in files.as_array().into_iter().flatten() {
        let np = f["filename"].as_str().unwrap_or_default();
        let op = f["previous_filename"].as_str().unwrap_or(np);
        let corpo = f["patch"].as_str().unwrap_or_default();
        raw.push((np.to_string(), op.to_string(), corpo.to_string()));
        let novo = match f["status"].as_str() {
            Some("added") => "new file mode 100644\n",
            _ => "",
        };
        patch.push_str(&format!(
            "diff --git a/{op} b/{np}\n{novo}--- a/{op}\n+++ b/{np}\n{corpo}"
        ));
        if !corpo.ends_with('\n') {
            patch.push('\n');
        }
    }
    (raw, patch)
}

/// Comentário de linha e comentário geral viram discussões como as do GitLab, que é o que a tela lê.
/// O id da discussão é o id do comentário-raiz, e é por ele que a resposta vai.
pub fn discussoes(linha: &Value, gerais: &Value) -> Value {
    let mut por_raiz: Vec<(u64, Vec<Value>)> = Vec::new();
    for c in linha.as_array().into_iter().flatten() {
        let id = c["id"].as_u64().unwrap_or_default();
        let raiz = c["in_reply_to_id"].as_u64().unwrap_or(id);
        let nota = json!({
            "id": id,
            "author": {"username": c["user"]["login"]},
            "body": c["body"],
            "created_at": c["created_at"],
            "system": false,
            "resolvable": true,
            // a REST do GitHub não diz se a conversa foi resolvida (só o GraphQL)
            "resolved": false,
            "position": {
                "new_path": c["path"],
                "old_path": c["path"],
                "new_line": c["line"].as_u64().or(c["original_line"].as_u64()),
            },
        });
        match por_raiz.iter_mut().find(|(r, _)| *r == raiz) {
            Some((_, notas)) => notas.push(nota),
            None => por_raiz.push((raiz, vec![nota])),
        }
    }
    let mut out: Vec<Value> = por_raiz
        .into_iter()
        .map(|(raiz, notas)| json!({"id": raiz.to_string(), "notes": notas}))
        .collect();
    for c in gerais.as_array().into_iter().flatten() {
        out.push(json!({
            "id": c["id"].as_u64().unwrap_or_default().to_string(),
            "notes": [{
                "id": c["id"],
                "author": {"username": c["user"]["login"]},
                "body": c["body"],
                "created_at": c["created_at"],
                "system": false,
                "resolvable": false,
                "resolved": false,
            }],
        }));
    }
    Value::Array(out)
}

/// As aprovações no formato do `/approvals`: quem aprovou e se fui eu.
pub fn aprovacoes(reviews: &Value, eu: &str) -> Value {
    let mut quem: Vec<Value> = Vec::new();
    let mut eu_aprovei = false;
    for r in reviews.as_array().into_iter().flatten() {
        if r["state"] != "APPROVED" {
            continue;
        }
        let login = r["user"]["login"].as_str().unwrap_or_default();
        if login == eu {
            eu_aprovei = true;
        }
        if !quem
            .iter()
            .any(|u| u["user"]["username"].as_str() == Some(login))
        {
            quem.push(json!({"user": {"username": login, "name": login}}));
        }
    }
    json!({
        "user_has_approved": eu_aprovei,
        "user_can_approve": !eu_aprovei,
        "approved_by": quem,
    })
}

/// O id da minha review aprovada, para poder desfazer (o GitHub não tem "unapprove": descarta a review).
pub fn minha_aprovacao(reviews: &Value, eu: &str) -> Option<u64> {
    reviews
        .as_array()?
        .iter()
        .rev()
        .find(|r| r["state"] == "APPROVED" && r["user"]["login"].as_str() == Some(eu))
        .and_then(|r| r["id"].as_u64())
}

/// Por que o Merge está apagado. `None` quando dá para mergear (ou agendar, se o CI ainda roda).
pub fn merge_blocker(detail: &Value) -> Option<&'static str> {
    if detail["state"] != "open" {
        return Some("o PR não está aberto");
    }
    if detail["draft"] == true {
        return Some("o PR é rascunho");
    }
    match detail["mergeable_state"].as_str().unwrap_or_default() {
        "clean" | "has_hooks" | "" => None,
        // o CI ainda roda ou falhou: dá para agendar com "merge quando passar"
        "unstable" | "pending" => None,
        "dirty" => Some("conflito com a branch de destino"),
        "behind" => Some("a branch está atrás do destino"),
        "blocked" => Some("falta aprovação ou um check obrigatório"),
        "draft" => Some("o PR é rascunho"),
        _ => Some("o GitHub não permite o merge agora"),
    }
}

/// O corpo do merge. O GitHub escolhe o método em vez de um `squash: bool`.
pub fn merge_body(sha: &str, squash: bool) -> Value {
    json!({
        "sha": sha,
        "merge_method": if squash { "squash" } else { "merge" },
    })
}

/// Uma review só com todos os comentários de linha: é o equivalente do `bulk_publish`, e também
/// é tudo ou nada — se um comentário não encaixa na linha, o GitHub recusa a review inteira.
pub fn review_body(sha: &str, geral: &str, comentarios: Vec<Value>) -> Value {
    let mut v = json!({"commit_id": sha, "event": "COMMENT", "comments": comentarios});
    if !geral.trim().is_empty() {
        v["body"] = json!(geral);
    }
    v
}

/// Um comentário de linha da review: linha removida é `LEFT`, linha nova é `RIGHT`.
pub fn comentario(file: &str, linha: Option<u32>, old: Option<u32>, body: &str) -> Value {
    match (linha, old) {
        (Some(l), _) => json!({"path": file, "line": l, "side": "RIGHT", "body": body}),
        (None, Some(o)) => json!({"path": file, "line": o, "side": "LEFT", "body": body}),
        _ => json!({"path": file, "subject_type": "file", "body": body}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_busca_vira_fila() {
        let busca = json!({"items": [{
            "number": 38, "title": "bump", "html_url": "https://github.com/o/r/pull/38",
            "repository_url": "https://api.github.com/repos/o/r",
            "user": {"login": "colega"}, "comments": 2, "updated_at": "2026-10-28T10:00:00Z"
        }]});
        let mut rs = rows(&busca);
        assert_eq!(rs[0].path, "o/r");
        assert_eq!(rs[0].iid, 38);
        assert_eq!(rs[0].who, "@colega");
        assert_eq!(rs[0].key(), "o/r#38", "a chave usa a marca da forja");
        completa(
            &mut rs[0],
            &json!({"head": {"sha": "abc", "ref": "feat/x"}, "base": {"ref": "main"}}),
        );
        assert_eq!((rs[0].sha.as_str(), rs[0].target.as_str()), ("abc", "main"));
    }

    #[test]
    fn comentarios_viram_discussoes() {
        let linha = json!([
            {"id": 1, "path": "a.ts", "line": 12, "body": "isso estoura",
             "user": {"login": "rev"}, "created_at": "t1"},
            {"id": 2, "path": "a.ts", "line": 12, "body": "concordo", "in_reply_to_id": 1,
             "user": {"login": "eu"}, "created_at": "t2"}
        ]);
        let gerais =
            json!([{"id": 9, "body": "subi o patch", "user": {"login": "eu"}, "created_at": "t3"}]);
        let d = discussoes(&linha, &gerais);
        let d = d.as_array().unwrap();
        assert_eq!(d.len(), 2, "a resposta entra na mesma discussão");
        assert_eq!(d[0]["id"], "1");
        assert_eq!(d[0]["notes"].as_array().unwrap().len(), 2);
        assert_eq!(d[0]["notes"][0]["position"]["new_path"], "a.ts");
        assert_eq!(d[0]["notes"][0]["author"]["username"], "rev");
        assert_eq!(d[1]["notes"][0]["resolvable"], false);
        // é o que a tela do card já sabe ler
        let c = crate::mr::comments(&json!(d), "autor");
        assert_eq!(c[0].at, "a.ts:12");
    }

    #[test]
    fn reviews_viram_aprovacoes() {
        let reviews = json!([
            {"id": 10, "state": "COMMENTED", "user": {"login": "x"}},
            {"id": 11, "state": "APPROVED", "user": {"login": "eu"}},
            {"id": 12, "state": "APPROVED", "user": {"login": "outro"}}
        ]);
        let a = aprovacoes(&reviews, "eu");
        assert_eq!(a["user_has_approved"], true);
        assert_eq!(a["user_can_approve"], false);
        assert_eq!(a["approved_by"].as_array().unwrap().len(), 2);
        assert_eq!(minha_aprovacao(&reviews, "eu"), Some(11));
        assert_eq!(minha_aprovacao(&reviews, "ninguem"), None);
    }

    #[test]
    fn arquivos_viram_patch() {
        let files = json!([
            {"filename": "a.ts", "status": "modified", "patch": "@@ -1 +1 @@\n-a\n+b"},
            {"filename": "novo.ts", "status": "added", "patch": "@@ -0,0 +1 @@\n+x\n"}
        ]);
        let (raw, patch) = arquivos(&files);
        assert_eq!(raw[0].0, "a.ts");
        assert!(patch.contains("diff --git a/a.ts b/a.ts"));
        assert!(patch.contains("new file mode 100644"));
        let fs = crate::diff::parse(&patch);
        assert_eq!(fs.len(), 2, "o parser de diff do Maestro lê o patch colado");
    }

    #[test]
    fn o_que_trava_o_merge() {
        let d = |estado: &str, s: &str| json!({"state": estado, "mergeable_state": s});
        assert_eq!(merge_blocker(&d("open", "clean")), None);
        assert_eq!(merge_blocker(&d("open", "unstable")), None);
        assert!(merge_blocker(&d("closed", "clean")).is_some());
        assert_eq!(
            merge_blocker(&d("open", "dirty")),
            Some("conflito com a branch de destino")
        );
        assert_eq!(
            merge_blocker(&json!({"state": "open", "draft": true})),
            Some("o PR é rascunho")
        );
        assert_eq!(merge_body("abc", true)["merge_method"], "squash");
    }

    #[test]
    fn comentario_de_linha_e_de_arquivo() {
        assert_eq!(comentario("a.ts", Some(3), None, "x")["side"], "RIGHT");
        assert_eq!(comentario("a.ts", None, Some(9), "x")["side"], "LEFT");
        assert_eq!(comentario("a.ts", None, None, "x")["subject_type"], "file");
        let r = review_body("sha", "", vec![comentario("a.ts", Some(1), None, "y")]);
        assert_eq!(r["event"], "COMMENT");
        assert!(r["body"].is_null(), "sem nota geral, sem corpo");
        assert_eq!(r["comments"].as_array().unwrap().len(), 1);
    }
}
