//! A engine de regras do Maestro: nativa, configurável e própria — nada de ESLint, prettier ou
//! gramática de terceiro. O núcleo não sabe o que é JavaScript: linguagem e regra entram por plugin.
//!
//! - [`cst`]: a árvore lossless que o lint e o formatter compartilham.
//! - [`regra`]: a regra declarativa, cidadã de primeira classe (`.maestro/regras/*.regra`).
//! - [`lang`]: o contrato do plugin de linguagem e a semente do `js-ts`.
//!
//! Três modos, um núcleo: daemon/LSP (editor), CLI (`maestro lint`, pre-commit e CI) e agente —
//! que é o CLI mesmo, saindo com 2 para o hook `PostToolUse` do Claude devolver o texto ao modelo.

pub mod cst;
pub mod lang;
pub mod regra;

use std::{collections::HashMap, path::Path};

use lang::{JsTs, Linguagem};
use regra::{Regra, Severidade};

/// O que ensina gente e LLM a escrever regra: vai embutido no binário, sai no `maestro lint --como`
/// e é o que o Maestro injeta no `--append-system-prompt` do agente.
pub const COMO_ESCREVER: &str = include_str!("../../assets/lint/REGRA.md");

/// As regras que vêm de fábrica. Pack do repo é adicional, nunca precisa substituir isto.
const NATIVAS: [&str; 1] = [include_str!("../../assets/lint/packs/js-base.regra")];

#[derive(Debug, Clone)]
pub struct Diagnostico {
    pub regra: String,
    pub severidade: Severidade,
    pub caminho: String,
    pub linha: usize,
    pub coluna: usize,
    pub trecho: String,
    pub porque: String,
    pub certo: Option<String>,
}

impl Diagnostico {
    /// O texto que vai para o modelo (e para o humano): onde, o quê, por quê e o certo.
    /// Sem o porquê o agente conserta a linha e repete o erro no arquivo seguinte.
    pub fn para_agente(&self) -> String {
        let mut s = format!(
            "{}:{}:{} {} {}\n  {}\n  porque: {}",
            self.caminho,
            self.linha,
            self.coluna,
            self.severidade,
            self.regra,
            self.trecho.trim(),
            self.porque
        );
        if let Some(c) = &self.certo {
            s.push_str(&format!("\n  certo: {c}"));
        }
        s
    }
}

pub struct Motor {
    regras: Vec<Regra>,
    linguagens: Vec<Box<dyn Linguagem>>,
}

impl Default for Motor {
    fn default() -> Self {
        Self::nativo()
    }
}

impl Motor {
    pub fn nativo() -> Self {
        let regras = NATIVAS
            .iter()
            .filter_map(|t| regra::parse(t).ok())
            .collect();
        Self {
            regras,
            linguagens: vec![Box::new(JsTs)],
        }
    }

    /// Carrega `<raiz>/.maestro/regras/*.regra`. Devolve o que não carregou, com o motivo: regra
    /// torta é erro visível, não regra que some.
    pub fn carregar(&mut self, raiz: &Path) -> Vec<String> {
        let dir = raiz.join(".maestro/regras");
        let mut erros = Vec::new();
        let mut arquivos: Vec<_> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "regra"))
            .collect();
        arquivos.sort();
        for p in arquivos {
            match std::fs::read_to_string(&p).map(|t| regra::parse(&t)) {
                Ok(Ok(r)) => self.regras.push(r),
                Ok(Err(e)) => erros.push(format!("{}: {e}", p.display())),
                Err(e) => erros.push(format!("{}: {e}", p.display())),
            }
        }
        erros
    }

    pub fn regras(&self) -> &[Regra] {
        &self.regras
    }

    pub fn seletores(&self) -> Vec<(&'static str, &'static [&'static str])> {
        self.linguagens
            .iter()
            .map(|l| (l.nome(), l.seletores()))
            .collect()
    }

    /// Uma passada na árvore: as regras são indexadas pelo seletor, e os fatos de um nó saem uma
    /// vez só, por mais regras que olhem para ele. 300 regras custam quase o mesmo que 10.
    pub fn checar(&self, caminho: &Path, fonte: &str) -> Vec<Diagnostico> {
        let Some(lang) = self.linguagens.iter().find(|l| l.atende(caminho)) else {
            return Vec::new();
        };
        let nome = caminho.to_string_lossy().replace('\\', "/");
        let mut por_seletor: HashMap<&str, Vec<&Regra>> = HashMap::new();
        for r in self.regras.iter().filter(|r| r.atende(&nome)) {
            por_seletor.entry(&r.no).or_default().push(r);
        }
        if por_seletor.is_empty() {
            return Vec::new();
        }
        let cst = lang.parse(fonte);
        let mut achados = Vec::new();
        cst.visita(|id, no| {
            let Some(regras) = por_seletor.get(no.kind) else {
                return;
            };
            let fatos = lang.fatos(&cst, id, fonte);
            for r in regras {
                if r.viola(&fatos) {
                    achados.push((r, no.span.clone()));
                }
            }
        });
        achados
            .into_iter()
            .filter_map(|(r, span)| {
                let (linha, coluna, texto) = lugar(fonte, span.start);
                // a válvula pede motivo: `regra-ok: <id> <motivo>`. Sem motivo não vale, e o
                // `maestro lint --ledger` lista quem suprimiu o quê.
                let suprimida = texto
                    .split_once("regra-ok:")
                    .map(|(_, resto)| resto.trim())
                    .is_some_and(|resto| {
                        resto
                            .strip_prefix(r.id.as_str())
                            .is_some_and(|m| !m.trim().is_empty())
                    });
                (!suprimida).then(|| Diagnostico {
                    regra: r.id.clone(),
                    severidade: r.severidade,
                    caminho: nome.clone(),
                    linha,
                    coluna,
                    trecho: texto.to_string(),
                    porque: r.porque.clone(),
                    certo: r.certo.clone(),
                })
            })
            .collect()
    }
}

/// Linha (1), coluna (1) e a linha inteira de um deslocamento em bytes.
fn lugar(fonte: &str, inicio: usize) -> (usize, usize, &str) {
    let antes = &fonte[..inicio];
    let linha = antes.matches('\n').count() + 1;
    let ini_linha = antes.rfind('\n').map_or(0, |i| i + 1);
    let fim_linha = fonte[inicio..]
        .find('\n')
        .map_or(fonte.len(), |i| inicio + i);
    (linha, inicio - ini_linha + 1, &fonte[ini_linha..fim_linha])
}

/// `maestro lint [--como|--seletores|--regras] <arquivo…>`: o mesmo caminho serve ao hook do agente,
/// ao pre-commit e ao CI. Sai com 2 quando achou algo, que é o que o `PostToolUse` entrega ao modelo.
pub fn cli(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("--como") => {
            println!("{COMO_ESCREVER}");
            return 0;
        }
        // a ferramenta de quem escreve regra (e de quem cresce a gramática): mostra os nós que
        // existem naquele arquivo e confere a invariante da árvore
        Some("--arvore") => {
            let Some(a) = args.get(1) else {
                eprintln!("maestro lint --arvore <arquivo>");
                return 1;
            };
            let p = Path::new(a);
            let motor = Motor::nativo();
            let Some(lang) = motor.linguagens.iter().find(|l| l.atende(p)) else {
                eprintln!("{a}: nenhum plugin de linguagem atende");
                return 1;
            };
            let Ok(fonte) = std::fs::read_to_string(p) else {
                eprintln!("{a}: não deu para ler");
                return 1;
            };
            let cst = lang.parse(&fonte);
            cst.visita(|id, no| {
                if !no.folha() {
                    println!("{} {:?} {:?}", no.kind, no.span, cst.texto(id, &fonte));
                }
            });
            let intacta = cst.reimprime_byte_a_byte(&fonte) == fonte;
            println!("byte a byte: {}", if intacta { "ok" } else { "QUEBRADA" });
            return i32::from(!intacta);
        }
        Some("--seletores") => {
            for (lang, sels) in Motor::nativo().seletores() {
                println!("{lang}: {}", sels.join(", "));
            }
            return 0;
        }
        Some("--regras") => {
            let mut m = Motor::nativo();
            let erros = m.carregar(Path::new("."));
            for r in m.regras() {
                println!("{} [{}] {}", r.id, r.severidade, r.no);
            }
            erros.iter().for_each(|e| eprintln!("regra torta: {e}"));
            return i32::from(!erros.is_empty());
        }
        Some("--instalar") => {
            return match instalar(Path::new(args.get(1).map_or(".", String::as_str))) {
                Ok(feito) => {
                    feito.iter().for_each(|l| println!("{l}"));
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            };
        }
        // o hook `PostToolUse` do Claude: o JSON da ferramenta vem no stdin, e o arquivo escrito
        // com ele. Sem script intermediário, que é o que costuma quebrar no Windows.
        Some("--hook") => {
            let mut entrada = String::new();
            let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut entrada);
            let v: serde_json::Value = serde_json::from_str(&entrada).unwrap_or_default();
            let t = &v["tool_input"];
            let Some(arquivo) = t["file_path"]
                .as_str()
                .or_else(|| t["notebook_path"].as_str())
            else {
                return 0;
            };
            return checa(&[arquivo.to_string()], None) * 2;
        }
        // o pre-commit: só o que está no índice, e só bloqueio reprova
        Some("--commit") => {
            let o = std::process::Command::new("git")
                .args(["diff", "--cached", "--name-only", "--diff-filter=ACM"])
                .output();
            let Ok(o) = o else {
                eprintln!("maestro lint --commit: git não respondeu");
                return 0;
            };
            let arquivos: Vec<String> = String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(String::from)
                .collect();
            return checa(&arquivos, Some(Severidade::Bloqueio));
        }
        _ => {}
    }
    checa(args, None) * 2
}

/// Checa os arquivos e devolve 1 quando achou o que importa (`barra`: só essa severidade reprova).
fn checa(arquivos: &[String], barra: Option<Severidade>) -> i32 {
    let mut motor = Motor::nativo();
    let raiz = std::env::current_dir().unwrap_or_default();
    for e in motor.carregar(&raiz) {
        eprintln!("regra torta: {e}");
    }
    let mut achou = false;
    for a in arquivos {
        let p = Path::new(a);
        let Ok(fonte) = std::fs::read_to_string(p) else {
            continue;
        };
        // a regra fala em caminho do repo: o hook entrega absoluto
        let rel = p.strip_prefix(&raiz).unwrap_or(p);
        for d in motor.checar(rel, &fonte) {
            eprintln!("{}", d.para_agente());
            achou |= barra.is_none_or(|b| d.severidade == b);
        }
    }
    i32::from(achou)
}

/// Põe a engine para funcionar no repo: o hook do agente, o pre-commit e a skill que ensina a
/// escrever regra. Nada é sobrescrito: o que já existe é dito, não trocado.
pub fn instalar(raiz: &Path) -> Result<Vec<String>, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_string_lossy().replace('\\', "/");
    let mut feito = Vec::new();
    let escreve = |p: &Path, conteudo: &str, feito: &mut Vec<String>| -> Result<(), String> {
        if p.exists() {
            feito.push(format!("{} já existe, não mexi", p.display()));
            return Ok(());
        }
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(p, conteudo).map_err(|e| e.to_string())?;
        feito.push(format!("{}", p.display()));
        Ok(())
    };

    // 1. o hook do agente, dentro do settings.json que já estiver lá
    let s = raiz.join(".claude/settings.json");
    let mut cfg: serde_json::Value = std::fs::read_to_string(&s)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let cmd = format!("\"{exe}\" lint --hook");
    let lista = cfg["hooks"]["PostToolUse"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if serde_json::Value::Array(lista.clone())
        .to_string()
        .contains("lint --hook")
    {
        feito.push(format!("{}: o hook já estava lá", s.display()));
    } else {
        let mut lista = lista;
        lista.push(serde_json::json!({
            "matcher": "Edit|Write|MultiEdit",
            "hooks": [{"type": "command", "command": cmd}],
        }));
        cfg["hooks"]["PostToolUse"] = serde_json::Value::Array(lista);
        if let Some(d) = s.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&s, serde_json::to_string_pretty(&cfg).unwrap_or_default())
            .map_err(|e| e.to_string())?;
        feito.push(format!("{} (hook PostToolUse)", s.display()));
    }

    // 2. o pre-commit, onde bloqueio reprova de verdade
    let hook = raiz.join(".git/hooks/pre-commit");
    if hook.parent().is_some_and(Path::exists) {
        escreve(
            &hook,
            &format!(
                "#!/bin/sh\n# posto por `maestro lint --instalar`\nexec \"{exe}\" lint --commit\n"
            ),
            &mut feito,
        )?;
        #[cfg(unix)]
        if hook.exists() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755));
        }
    } else {
        feito.push("sem .git/hooks: o pre-commit fica para quando for um repo".into());
    }

    // 3. o que ensina a LLM a escrever regra, onde ela já procura
    escreve(
        &raiz.join(".claude/skills/regra-maestro/SKILL.md"),
        &format!(
            "---\nname: regra-maestro\ndescription: Escrever ou ajustar uma regra da engine do Maestro \
             (.maestro/regras/*.regra). Use quando pedirem regra de lint, padrão do repo ou \
             \"o Maestro reclamou\".\n---\n\n{COMO_ESCREVER}"
        ),
        &mut feito,
    )?;
    escreve(
        &raiz.join(".maestro/regras/.gitkeep"),
        "# as regras deste repo moram aqui (*.regra)\n",
        &mut feito,
    )?;
    Ok(feito)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DINHEIRO: &str = "\
regra dinheiro-inteiro
severidade bloqueio
quando arquivo src/domain/**
quando no declaracao-de-campo
proibido tipo number Decimal
a_menos_que nome_termina_em Cents Micros
porque Valor é inteiro em unidade mínima: em ponto flutuante o total não fecha.
certo precoCents: number
";

    fn motor() -> Motor {
        Motor {
            regras: vec![regra::parse(DINHEIRO).unwrap()],
            linguagens: vec![Box::new(JsTs)],
        }
    }

    #[test]
    fn acha_o_dinheiro_em_ponto_flutuante_e_respeita_a_valvula() {
        let fonte = "class Venda {\n  precoCents: number\n  taxa: number\n  frete: Decimal\n}\n";
        let ds = motor().checar(Path::new("src/domain/venda.ts"), fonte);
        assert_eq!(
            ds.iter().map(|d| d.linha).collect::<Vec<_>>(),
            [3, 4],
            "o campo em Cents está isento"
        );
        assert!(ds[0].para_agente().contains("porque: Valor é inteiro"));
        assert_eq!(ds[0].severidade, Severidade::Bloqueio);

        // fora do escopo da regra
        assert!(
            motor()
                .checar(Path::new("src/infra/http.ts"), fonte)
                .is_empty()
        );
        // outra linguagem: o plugin não atende, ninguém reclama
        assert!(motor().checar(Path::new("a/b.cs"), fonte).is_empty());

        let com_valvula = "class Venda {\n  taxa: number // regra-ok: dinheiro-inteiro taxa legada do fiscal\n}\n";
        assert!(
            motor()
                .checar(Path::new("src/domain/venda.ts"), com_valvula)
                .is_empty()
        );
        let sem_motivo = "class Venda {\n  taxa: number // regra-ok: dinheiro-inteiro\n}\n";
        assert_eq!(
            motor()
                .checar(Path::new("src/domain/venda.ts"), sem_motivo)
                .len(),
            1,
            "supressão sem motivo não vale"
        );
    }

    #[test]
    fn as_regras_nativas_carregam() {
        let m = Motor::nativo();
        assert!(!m.regras().is_empty(), "o pack nativo tem de carregar");
        assert!(COMO_ESCREVER.contains("quando no"));
    }
}
