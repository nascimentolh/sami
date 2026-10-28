//! A regra declarativa: cidadã de primeira classe, e o contrato mais importante do subsistema.
//!
//! Uma diretiva por linha, `chave args…`, sem indentação, sem aspas e sem escape — o formato
//! existe para ser escrito rápido por gente e por LLM, e para errar alto quando está errado
//! (chave desconhecida é erro com número de linha, não silêncio).
//!
//! ```text
//! regra dinheiro-inteiro
//! severidade bloqueio
//! quando arquivo src/domain/** src/app/**
//! quando no declaracao-de-campo
//! proibido tipo number Decimal decimal double
//! a_menos_que nome_termina_em Cents Micros PercentMicros
//! porque Valor é inteiro em unidade mínima: em ponto flutuante o total não fecha.
//! errado preco: number
//! certo precoCents: number
//! ```
//!
//! A regra nunca fala de AST: ela cita **fatos** (`tipo`, `nome`) que o plugin de linguagem
//! traduz da árvore. É o que a mantém legível e estável quando a gramática cresce.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severidade {
    /// na escrita: o agente recebe e conserta no mesmo turno
    Aviso,
    /// no commit: reprova
    Bloqueio,
}

impl fmt::Display for Severidade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severidade::Aviso => "aviso",
            Severidade::Bloqueio => "bloqueio",
        })
    }
}

/// O sufixo da chave é o operador: `nome_termina_em Cents` em vez de um campo `op:` à parte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Igual,
    TerminaEm,
    ComecaCom,
    Contem,
}

const SUFIXOS: [(&str, Op); 3] = [
    ("_termina_em", Op::TerminaEm),
    ("_comeca_com", Op::ComecaCom),
    ("_contem", Op::Contem),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Teste {
    pub fato: String,
    pub op: Op,
    pub valores: Vec<String>,
}

impl Teste {
    fn de(chave: &str, valores: Vec<String>) -> Self {
        let (fato, op) = SUFIXOS
            .iter()
            .find_map(|(s, op)| chave.strip_suffix(s).map(|f| (f.to_string(), *op)))
            .unwrap_or_else(|| (chave.to_string(), Op::Igual));
        Self { fato, op, valores }
    }

    fn casa(&self, valor: &str) -> bool {
        self.valores.iter().any(|v| match self.op {
            Op::Igual => valor == v,
            Op::TerminaEm => valor.ends_with(v.as_str()),
            Op::ComecaCom => valor.starts_with(v.as_str()),
            Op::Contem => valor.contains(v.as_str()),
        })
    }
}

/// Um conjunto de testes: casa quando **algum** fato citado casa (a regra pede uma coisa de cada vez;
/// quem precisa de duas escreve duas regras, e o diagnóstico fica específico).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pred(pub Vec<Teste>);

impl Pred {
    pub fn vazio(&self) -> bool {
        self.0.is_empty()
    }

    fn casa(&self, fatos: &crate::lint::lang::Fatos) -> bool {
        self.0
            .iter()
            .any(|t| fatos.valores(&t.fato).any(|v| t.casa(v)))
    }
}

#[derive(Debug, Clone)]
pub struct Regra {
    pub id: String,
    pub severidade: Severidade,
    /// globs; vazio = qualquer arquivo da linguagem
    pub arquivos: Vec<String>,
    /// o seletor que o núcleo indexa para despachar numa passada só
    pub no: String,
    pub proibido: Pred,
    pub exigido: Pred,
    pub a_menos_que: Pred,
    pub porque: String,
    pub errado: Option<String>,
    pub certo: Option<String>,
}

impl Regra {
    /// Se o nó viola a regra, considerando os fatos que o plugin extraiu dele.
    pub fn viola(&self, fatos: &crate::lint::lang::Fatos) -> bool {
        if self.a_menos_que.casa(fatos) {
            return false;
        }
        if !self.proibido.vazio() && self.proibido.casa(fatos) {
            return true;
        }
        !self.exigido.vazio() && !self.exigido.casa(fatos)
    }

    pub fn atende(&self, caminho: &str) -> bool {
        self.arquivos.is_empty() || self.arquivos.iter().any(|g| casa_glob(g, caminho))
    }
}

/// Lê um arquivo `.regra`. Erro com número de linha: regra que não carrega é problema de quem
/// escreveu, não vale carregar meia regra e seguir.
pub fn parse(texto: &str) -> Result<Regra, String> {
    let mut r = Regra {
        id: String::new(),
        severidade: Severidade::Aviso,
        arquivos: Vec::new(),
        no: String::new(),
        proibido: Pred::default(),
        exigido: Pred::default(),
        a_menos_que: Pred::default(),
        porque: String::new(),
        errado: None,
        certo: None,
    };
    for (i, linha) in texto.lines().enumerate() {
        let n = i + 1;
        let linha = linha.trim();
        if linha.is_empty() || linha.starts_with('#') {
            continue;
        }
        let (chave, resto) = linha.split_once(char::is_whitespace).unwrap_or((linha, ""));
        let resto = resto.trim();
        let args = || {
            resto
                .split_whitespace()
                .map(String::from)
                .collect::<Vec<_>>()
        };
        let um = |o: &str| -> Result<String, String> {
            match resto.is_empty() {
                true => Err(format!("linha {n}: `{o}` sem valor")),
                false => Ok(resto.to_string()),
            }
        };
        match chave {
            "regra" => r.id = um("regra")?,
            "severidade" => {
                r.severidade = match resto {
                    "aviso" => Severidade::Aviso,
                    "bloqueio" => Severidade::Bloqueio,
                    _ => {
                        return Err(format!(
                            "linha {n}: severidade é aviso ou bloqueio, não `{resto}`"
                        ));
                    }
                }
            }
            "quando" => {
                let (o, valores) = resto
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| format!("linha {n}: `quando` pede `arquivo` ou `no`"))?;
                match o {
                    "arquivo" => {
                        r.arquivos = valores.split_whitespace().map(String::from).collect()
                    }
                    "no" => r.no = valores.trim().to_string(),
                    _ => {
                        return Err(format!(
                            "linha {n}: `quando {o}` não existe (use arquivo ou no)"
                        ));
                    }
                }
            }
            "proibido" | "exigido" | "a_menos_que" => {
                let (fato, valores) = resto
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| format!("linha {n}: `{chave}` pede fato e valores"))?;
                let t = Teste::de(fato, valores.split_whitespace().map(String::from).collect());
                match chave {
                    "proibido" => r.proibido.0.push(t),
                    "exigido" => r.exigido.0.push(t),
                    _ => r.a_menos_que.0.push(t),
                }
            }
            "porque" => r.porque = um("porque")?,
            "errado" => r.errado = Some(um("errado")?),
            "certo" => r.certo = Some(um("certo")?),
            _ => return Err(format!("linha {n}: chave `{chave}` não existe")),
        }
        let _ = args;
    }
    if r.id.is_empty() {
        return Err("falta `regra <id>`".into());
    }
    if r.no.is_empty() {
        return Err(format!("{}: falta `quando no <seletor>`", r.id));
    }
    // sem porquê a regra não serve ao agente nem a quem lê o diagnóstico às 3 da manhã
    if r.porque.is_empty() {
        return Err(format!("{}: falta `porque <motivo>`", r.id));
    }
    if r.proibido.vazio() && r.exigido.vazio() {
        return Err(format!("{}: falta `proibido` ou `exigido`", r.id));
    }
    Ok(r)
}

/// Glob de caminho com `*` (um segmento) e `**` (qualquer profundidade). O suficiente para `paths:`;
/// cresce quando aparecer caso que não cabe.
pub fn casa_glob(glob: &str, caminho: &str) -> bool {
    fn casa(p: &[&str], c: &[&str]) -> bool {
        match p.split_first() {
            None => c.is_empty(),
            Some((&"**", resto)) => (0..=c.len()).any(|i| casa(resto, &c[i..])),
            Some((seg, resto)) => match c.split_first() {
                Some((primeiro, c_resto)) if casa_seg(seg, primeiro) => casa(resto, c_resto),
                _ => false,
            },
        }
    }
    fn casa_seg(p: &str, s: &str) -> bool {
        match p.split_once('*') {
            None => p == s,
            Some((a, b)) => s.len() >= a.len() + b.len() && s.starts_with(a) && s.ends_with(b),
        }
    }
    fn norm(s: &str) -> Vec<&str> {
        s.split(['/', '\\'])
            .filter(|p| !p.is_empty() && *p != ".")
            .collect()
    }
    casa(&norm(glob), &norm(caminho))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DINHEIRO: &str = "\
# a regra do Tessel, como ela é hoje em prosa
regra dinheiro-inteiro
severidade bloqueio
quando arquivo src/domain/** src/app/**
quando no declaracao-de-campo
proibido tipo number Decimal decimal double
a_menos_que nome_termina_em Cents Micros PercentMicros
porque Valor é inteiro em unidade mínima: em ponto flutuante o total não fecha.
errado preco: number
certo precoCents: number
";

    #[test]
    fn le_a_regra_e_recusa_a_torta() {
        let r = parse(DINHEIRO).unwrap();
        assert_eq!(r.id, "dinheiro-inteiro");
        assert_eq!(r.severidade, Severidade::Bloqueio);
        assert_eq!(r.no, "declaracao-de-campo");
        assert_eq!(r.proibido.0[0].fato, "tipo");
        assert_eq!(r.a_menos_que.0[0].op, Op::TerminaEm);
        assert_eq!(r.certo.as_deref(), Some("precoCents: number"));
        assert!(r.atende("src/domain/venda.ts"));
        assert!(!r.atende("src/infra/http.ts"));

        for (texto, erro) in [
            ("regra x\nquando no y\nproibido tipo a\n", "porque"),
            ("regra x\nporque p\nproibido tipo a\n", "quando no"),
            ("regra x\nquando no y\nporque p\n", "proibido"),
            ("regra x\nsevridade aviso\n", "não existe"),
            ("regra x\nseveridade talvez\n", "severidade é aviso"),
        ] {
            let e = parse(texto).unwrap_err();
            assert!(e.contains(erro), "{texto:?} deu {e:?}");
        }
    }

    #[test]
    fn glob_de_caminho() {
        assert!(casa_glob("src/**/*.ts", "src/a/b/c.ts"));
        assert!(casa_glob("src/**", "src/a"));
        assert!(casa_glob("**/*.cs", "TesselHub.Domain/Entity/Venda.cs"));
        assert!(!casa_glob("src/*/x.ts", "src/a/b/x.ts"));
        assert!(casa_glob("src/*.ts", "src\\a.ts"), "o Windows manda \\");
    }
}
