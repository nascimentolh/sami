use std::process::Command;

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

#[derive(Debug, PartialEq, Deserialize)]
pub struct Issue {
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub criteria: String,
}

/// `PROJ-123`
pub fn is_key(k: &str) -> bool {
    let Some((proj, n)) = k.split_once('-') else {
        return false;
    };
    proj.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && proj
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        && !n.is_empty()
        && n.chars().all(|c| c.is_ascii_digit())
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Card {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    #[serde(default)]
    pub squad: String,
    pub title: String,
    #[serde(default)]
    pub desc: String,
    /// `https://<site>.atlassian.net`, do getAccessibleAtlassianResources
    #[serde(default)]
    pub site: String,
    #[serde(default)]
    pub subs: Vec<Sub>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Sub {
    pub key: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub sp: Option<f64>,
    pub status: String,
    /// os status de destino que a sub-task alcança agora
    #[serde(default)]
    pub transitions: Vec<String>,
}

#[derive(Deserialize)]
struct Created {
    key: String,
}

const CLOUD: &str = "Chame getAccessibleAtlassianResources para obter o cloudId (e a url do site)";
const ERR: &str =
    "se alguma ferramenta der erro, responda SOMENTE {\"error\":\"<mensagem da ferramenta>\"}";

/// Um `claude -p` curto com o MCP do Atlassian do próprio Claude Code; devolve o JSON da resposta.
/// ponytail: haiku custa ~US$ 0,01 e ~7 s por chamada; trocar pela API do Jira se ficar frequente
fn run<T: DeserializeOwned>(prompt: String, tools: &[&str]) -> Result<T, String> {
    let tools: Vec<String> = ["getAccessibleAtlassianResources"]
        .iter()
        .chain(tools)
        .map(|t| format!("mcp__claude_ai_Atlassian_MCP__{t}"))
        .collect();
    let out = Command::new("claude")
        .arg("-p")
        .arg(prompt)
        .args(["--model", "haiku", "--output-format", "json"])
        .arg(format!("--allowedTools={}", tools.join(",")))
        .current_dir(std::env::temp_dir())
        .output()
        .map_err(|e| format!("claude: {e}"))?;
    let reply: Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| "resposta inválida do claude".to_string())?;
    parse(reply["result"].as_str().unwrap_or_default())
}

pub fn fetch(key: &str) -> Result<Issue, String> {
    run(
        format!(
            "{CLOUD} e depois getJiraIssue com issueIdOrKey {key}. \
             Responda SOMENTE um JSON {{\"type\":\"\",\"title\":\"\",\"criteria\":\"\"}} com o tipo, o título e os \
             critérios de aceite em uma frase; {ERR}."
        ),
        &["getJiraIssue"],
    )
}

/// O card com as sub-tasks, os SP (`customfield_10004`), a squad (`customfield_18419`) e as transições de cada uma.
pub fn card(key: &str) -> Result<Card, String> {
    run(
        format!(
            "{CLOUD}. Chame getJiraIssue com issueIdOrKey {key} (campos summary, description, issuetype, status, \
             subtasks e customfield_18419, que é a squad). Para cada sub-task, getJiraIssue dela (summary, issuetype, \
             status e customfield_10004, que são os story points) e executeRead com name listJiraIssueTransitions e \
             inputs {{\"issueIdOrKey\":\"<key da sub-task>\"}}. Responda SOMENTE um JSON \
             {{\"key\":\"{key}\",\"type\":\"\",\"status\":\"\",\"squad\":\"\",\"title\":\"\",\"desc\":\"\",\
             \"site\":\"https://<site>.atlassian.net\",\"subs\":[{{\"key\":\"\",\"type\":\"\",\"title\":\"\",\"sp\":2,\
             \"status\":\"\",\"transitions\":[\"<nome do status de destino de cada transição>\"]}}]}} com a descrição \
             em texto puro e sp null quando não houver; {ERR}."
        ),
        &["getJiraIssue", "executeRead"],
    )
}

pub fn save(key: &str, title: &str, desc: &str) -> Result<(), String> {
    let fields = serde_json::json!({ "summary": title, "description": desc });
    run::<Value>(
        format!(
            "{CLOUD} e depois editJiraIssue com issueIdOrKey {key} e fields {fields} (descrição em texto puro). \
             Responda SOMENTE {{\"ok\":true}}; {ERR}."
        ),
        &["editJiraIssue"],
    )
    .map(|_| ())
}

/// Leva a issue à transição cujo status de destino é `to`.
pub fn transition(key: &str, to: &str) -> Result<(), String> {
    let to = serde_json::to_string(to).unwrap_or_default();
    run::<Value>(
        format!(
            "{CLOUD}. Chame executeRead com name listJiraIssueTransitions e inputs {{\"issueIdOrKey\":\"{key}\"}}, \
             escolha a transição cujo status de destino (to.name) é {to} e chame transitionJiraIssue com issueIdOrKey \
             {key} e o id dela. Responda SOMENTE {{\"ok\":true}}; se nenhuma transição levar a esse status, \
             {{\"error\":\"sem transição para {}\"}}; {ERR}.",
            to.trim_matches('"')
        ),
        &["executeRead", "transitionJiraIssue"],
    )
    .map(|_| ())
}

/// Cria o card no projeto, atribuído a você; devolve a key.
pub fn create(project: &str, kind: &str, title: &str) -> Result<String, String> {
    let title = serde_json::to_string(title).unwrap_or_default();
    run::<Created>(
        format!(
            "{CLOUD} e atlassianUserInfo para o seu accountId. Chame createJiraIssue com projectKey {project}, \
             issueTypeName {kind}, summary {title} e assignee_account_id com o seu accountId. \
             Responda SOMENTE {{\"key\":\"<key criada>\"}}; {ERR}."
        ),
        &["atlassianUserInfo", "createJiraIssue"],
    )
    .map(|c| c.key)
}

fn parse<T: DeserializeOwned>(result: &str) -> Result<T, String> {
    let (a, b) = (result.find('{'), result.rfind('}'));
    let json = match (a, b) {
        (Some(a), Some(b)) if b > a => &result[a..=b],
        _ => return Err("o Claude não devolveu o JSON".into()),
    };
    let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if let Some(e) = v["error"].as_str() {
        return Err(e.to_string());
    }
    serde_json::from_value(v).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chave_e_resposta() {
        assert!(is_key("PRD-171500"));
        assert!(!is_key("prd-1") && !is_key("PRD") && !is_key("PRD-x"));
        let ok = "```json\n{\"type\":\"Story\",\"title\":\"Paginação\",\"criteria\":\"page e size\"}\n```";
        assert_eq!(parse::<Issue>(ok).unwrap().title, "Paginação");
        assert_eq!(
            parse::<Issue>("{\"error\":\"Issue \\\"PRD-1\\\" not found\"}").unwrap_err(),
            "Issue \"PRD-1\" not found"
        );
    }

    #[test]
    fn card_key_e_ok() {
        let reply = r#"Pronto:
{"key":"PRD-1","type":"Story","status":"Em andamento","squad":"Squad Bots","title":"Paginação","desc":"page e size",
"site":"https://x.atlassian.net","subs":[{"key":"PRD-2","type":"Desenvolvimento","title":"[bots-api] paginar","sp":2,
"status":"Building","transitions":["Code Review","Pull Request"]},{"key":"PRD-3","type":"Qualidade e Testes","title":"e2e","sp":null,"status":"Tarefas pendentes"}]}"#;
        let c = parse::<Card>(reply).unwrap();
        assert_eq!((c.squad.as_str(), c.subs.len()), ("Squad Bots", 2));
        assert_eq!(c.subs[0].sp, Some(2.));
        assert_eq!(c.subs[0].transitions, ["Code Review", "Pull Request"]);
        assert_eq!((c.subs[1].sp, c.subs[1].transitions.len()), (None, 0));
        assert_eq!(
            parse::<Created>("{\"key\":\"PRD-9\"}").unwrap().key,
            "PRD-9"
        );
        assert!(parse::<Value>("```json\n{\"ok\":true}\n```").is_ok());
        assert_eq!(
            parse::<Value>("{\"error\":\"sem transição para Pull Request\"}").unwrap_err(),
            "sem transição para Pull Request"
        );
        assert!(parse::<Value>("não consegui").is_err());
    }
}
