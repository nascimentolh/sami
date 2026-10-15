use std::process::Command;

use serde::Deserialize;
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

/// Lê a issue pelo MCP do Atlassian do próprio Claude Code (o mesmo do protótipo), num `claude -p` curto.
/// ponytail: haiku custa ~US$ 0,01 e ~7 s por busca; trocar pela API do Jira se ficar frequente
pub fn fetch(key: &str) -> Result<Issue, String> {
    let prompt = format!(
        "Chame getAccessibleAtlassianResources para obter o cloudId e depois getJiraIssue com issueIdOrKey {key}. \
         Responda SOMENTE um JSON {{\"type\":\"\",\"title\":\"\",\"criteria\":\"\"}} com o tipo, o título e os \
         critérios de aceite em uma frase; se a ferramenta der erro, {{\"error\":\"<mensagem da ferramenta>\"}}."
    );
    let out = Command::new("claude")
        .arg("-p")
        .arg(prompt)
        .args(["--model", "haiku", "--output-format", "json"])
        .arg("--allowedTools=mcp__claude_ai_Atlassian_MCP__getJiraIssue,mcp__claude_ai_Atlassian_MCP__getAccessibleAtlassianResources")
        .current_dir(std::env::temp_dir())
        .output()
        .map_err(|e| format!("claude: {e}"))?;
    let reply: Value = serde_json::from_slice(&out.stdout)
        .map_err(|_| "resposta inválida do claude".to_string())?;
    parse(reply["result"].as_str().unwrap_or_default())
}

fn parse(result: &str) -> Result<Issue, String> {
    let (a, b) = (result.find('{'), result.rfind('}'));
    let json = match (a, b) {
        (Some(a), Some(b)) if b > a => &result[a..=b],
        _ => return Err("o Claude não devolveu a issue".into()),
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
        assert_eq!(parse(ok).unwrap().title, "Paginação");
        assert_eq!(
            parse("{\"error\":\"Issue \\\"PRD-1\\\" not found\"}").unwrap_err(),
            "Issue \"PRD-1\" not found"
        );
    }
}
