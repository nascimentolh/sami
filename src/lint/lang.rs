//! O contrato do plugin de linguagem, e a semente do `js-ts`.
//!
//! O plugin entrega duas coisas: a CST (lexer + parser próprios, sem gramática de terceiro) e os
//! **fatos** de cada nó. A regra declarativa só fala de fato, então crescer a gramática não
//! reescreve regra nenhuma.

use std::path::Path;

use super::cst::{Builder, Cst, Id};

/// Os fatos de um nó. Lista e não mapa: um nó pode ter o mesmo fato várias vezes
/// (`chama`, `importa_de`) e a regra casa com qualquer um deles.
#[derive(Debug, Default)]
pub struct Fatos(pub Vec<(&'static str, String)>);

impl Fatos {
    pub fn valores(&self, fato: &str) -> impl Iterator<Item = &str> {
        self.0
            .iter()
            .filter(move |(f, _)| *f == fato)
            .map(|(_, v)| v.as_str())
    }
}

pub trait Linguagem: Send + Sync {
    fn nome(&self) -> &'static str;
    fn atende(&self, caminho: &Path) -> bool;
    fn parse(&self, fonte: &str) -> Cst;
    fn fatos(&self, cst: &Cst, id: Id, fonte: &str) -> Fatos;
    /// Os seletores que este plugin produz: o `maestro lint --seletores` os lista para quem
    /// (ou o que) vai escrever regra.
    fn seletores(&self) -> &'static [&'static str];
}

pub struct JsTs;

const CAMPO: &str = "declaracao-de-campo";

impl Linguagem for JsTs {
    fn nome(&self) -> &'static str {
        "js-ts"
    }

    fn atende(&self, caminho: &Path) -> bool {
        matches!(
            caminho.extension().and_then(|e| e.to_str()),
            Some("ts" | "tsx" | "js" | "jsx" | "mts" | "cts")
        )
    }

    // ponytail: semente do parser. Reconhece trivia, identificador, número, texto, pontuação e
    // agrupa `nome: tipo` em `declaracao-de-campo` — o bastante para a primeira fatia atravessar
    // ponta a ponta. A gramática inteira (classe, função, genérico, JSX, decorator) entra por
    // fatia, sem mexer nas regras, porque elas falam de fato e não de árvore.
    fn parse(&self, fonte: &str) -> Cst {
        let tokens = tokens(fonte);
        let mut b = Builder::new("arquivo");
        let mut i = 0;
        while i < tokens.len() {
            match campo(&tokens, i, fonte) {
                Some(fim) => {
                    b.abre(CAMPO, tokens[i].span.start);
                    for t in &tokens[i..fim] {
                        b.token(t.kind, t.span.clone());
                    }
                    b.fecha();
                    i = fim;
                }
                None => {
                    b.token(tokens[i].kind, tokens[i].span.clone());
                    i += 1;
                }
            }
        }
        b.fim()
    }

    fn fatos(&self, cst: &Cst, id: Id, fonte: &str) -> Fatos {
        let mut f = Fatos::default();
        if cst.no(id).kind != CAMPO {
            return f;
        }
        let filhos = &cst.no(id).filhos;
        if let Some(nome) = filhos.iter().find(|c| cst.no(**c).kind == "ident") {
            f.0.push(("nome", cst.texto(*nome, fonte).to_string()));
        }
        // o tipo é o que vem depois dos dois-pontos, sem a trivia das pontas
        if let Some(pos) = filhos
            .iter()
            .position(|c| cst.texto(*c, fonte) == ":" && cst.no(*c).kind == "punct")
        {
            let tipo: String = filhos[pos + 1..]
                .iter()
                .filter(|c| cst.no(**c).kind != "trivia")
                .map(|c| cst.texto(*c, fonte))
                .collect();
            if !tipo.is_empty() {
                f.0.push(("tipo", tipo));
            }
        }
        f
    }

    fn seletores(&self) -> &'static [&'static str] {
        &[CAMPO]
    }
}

#[derive(Debug)]
struct Token {
    kind: &'static str,
    span: std::ops::Range<usize>,
}

/// Onde o campo termina (índice exclusivo), se `i` começa um `nome: tipo` (ou `nome?: tipo`).
fn campo(tokens: &[Token], i: usize, fonte: &str) -> Option<usize> {
    if tokens[i].kind != "ident" {
        return None;
    }
    let texto = |t: &Token| &fonte[t.span.clone()];
    let pula = |j: &mut usize| {
        while tokens.get(*j).is_some_and(|t| t.kind == "trivia") {
            *j += 1;
        }
    };
    let mut j = i + 1;
    pula(&mut j);
    if texto(tokens.get(j)?) == "?" {
        j += 1;
        pula(&mut j);
    }
    if texto(tokens.get(j)?) != ":" {
        return None;
    }
    j += 1;
    pula(&mut j);
    // o tipo é o que vem colado depois: nome, genérico, união. Trivia encerra — num seed é melhor
    // perder o tipo que atravessa linha do que engolir o resto do arquivo.
    let mut fim = j;
    while let Some(t) = tokens.get(fim) {
        match t.kind {
            "ident" | "numero" | "texto" => fim += 1,
            "punct" if matches!(texto(t), "." | "<" | ">" | "[" | "]" | "|" | "&") => fim += 1,
            _ => break,
        }
    }
    (fim > j).then_some(fim)
}

fn tokens(fonte: &str) -> Vec<Token> {
    let b = fonte.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
    while i < b.len() {
        let inicio = i;
        let kind = match b[i] {
            c if c.is_ascii_whitespace() => {
                while i < b.len() && b[i].is_ascii_whitespace() {
                    i += 1;
                }
                "trivia"
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                "trivia"
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(b.len());
                "trivia"
            }
            c if c.is_ascii_digit() => {
                while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.' || b[i] == b'_') {
                    i += 1;
                }
                "numero"
            }
            q @ (b'"' | b'\'' | b'`') => {
                i += 1;
                while i < b.len() && b[i] != q {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i = (i + 1).min(b.len());
                "texto"
            }
            c if ident(c) || !c.is_ascii() => {
                while i < b.len() && (ident(b[i]) || !b[i].is_ascii()) {
                    i += 1;
                }
                "ident"
            }
            _ => {
                i += 1;
                "punct"
            }
        };
        out.push(Token {
            kind,
            span: inicio..i,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_o_campo_tipado_e_devolve_os_fatos() {
        let fonte = "class Venda {\n  // comentário\n  precoCents: number\n  taxa: Decimal\n}\n";
        let l = JsTs;
        let cst = l.parse(fonte);
        assert_eq!(
            cst.reimprime_byte_a_byte(fonte),
            fonte,
            "a árvore tem de ser lossless"
        );
        let mut campos = Vec::new();
        cst.visita(|id, no| {
            if no.kind == CAMPO {
                let f = l.fatos(&cst, id, fonte);
                campos.push((
                    f.valores("nome").next().unwrap_or("").to_string(),
                    f.valores("tipo").next().unwrap_or("").to_string(),
                ));
            }
        });
        assert_eq!(
            campos,
            [
                ("precoCents".to_string(), "number".to_string()),
                ("taxa".to_string(), "Decimal".to_string()),
            ]
        );
        assert!(l.atende(Path::new("a/b.tsx")) && !l.atende(Path::new("a/b.cs")));
    }
}
