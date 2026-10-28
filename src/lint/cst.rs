//! A árvore que o lint e, depois, o formatter compartilham.
//!
//! Invariante: **todo byte do arquivo pertence a exatamente um token**, inclusive espaço e
//! comentário. Os nós só agrupam tokens. É isso que deixa a mesma árvore servir aos dois:
//! reimprimir a raiz devolve o arquivo byte a byte (`reimprime_byte_a_byte`), e o formatter
//! pode decidir a disposição sem nunca perder o que estava escrito.

use std::ops::Range;

pub type Id = usize;

/// `kind` é o nome do nó que a regra cita em `quando no`: vem do plugin de linguagem, não do núcleo.
#[derive(Debug, Clone)]
pub struct No {
    pub kind: &'static str,
    pub span: Range<usize>,
    pub filhos: Vec<Id>,
}

impl No {
    pub fn folha(&self) -> bool {
        self.filhos.is_empty()
    }
}

#[derive(Debug, Default)]
pub struct Cst {
    nos: Vec<No>,
}

impl Cst {
    pub fn raiz(&self) -> Id {
        0
    }

    pub fn no(&self, id: Id) -> &No {
        &self.nos[id]
    }

    pub fn texto<'f>(&self, id: Id, fonte: &'f str) -> &'f str {
        &fonte[self.nos[id].span.clone()]
    }

    /// Pré-ordem, uma passada só: é sobre ela que o despacho das regras roda.
    pub fn visita(&self, mut f: impl FnMut(Id, &No)) {
        if self.nos.is_empty() {
            return;
        }
        let mut pilha = vec![self.raiz()];
        while let Some(id) = pilha.pop() {
            let no = &self.nos[id];
            f(id, no);
            pilha.extend(no.filhos.iter().rev());
        }
    }

    /// Só as folhas, na ordem: tem de sair igual ao arquivo de entrada.
    pub fn reimprime_byte_a_byte(&self, fonte: &str) -> String {
        let mut out = String::with_capacity(fonte.len());
        self.visita(|id, no| {
            if no.folha() {
                out.push_str(self.texto(id, fonte));
            }
        });
        out
    }
}

/// Construtor de árvore: `abre`/`fecha` para os nós, `token` para as folhas.
#[derive(Debug, Default)]
pub struct Builder {
    cst: Cst,
    abertos: Vec<Id>,
}

impl Builder {
    pub fn new(kind: &'static str) -> Self {
        let mut b = Self::default();
        b.abre(kind, 0);
        b
    }

    pub fn abre(&mut self, kind: &'static str, inicio: usize) -> Id {
        let id = self.cst.nos.len();
        self.cst.nos.push(No {
            kind,
            span: inicio..inicio,
            filhos: Vec::new(),
        });
        if let Some(pai) = self.abertos.last().copied() {
            self.cst.nos[pai].filhos.push(id);
        }
        self.abertos.push(id);
        id
    }

    pub fn token(&mut self, kind: &'static str, span: Range<usize>) {
        let id = self.cst.nos.len();
        self.cst.nos.push(No {
            kind,
            span: span.clone(),
            filhos: Vec::new(),
        });
        if let Some(pai) = self.abertos.last().copied() {
            self.cst.nos[pai].filhos.push(id);
            // o nó cresce com o que entra: o span do pai é o dos filhos, sempre
            for a in &self.abertos {
                self.cst.nos[*a].span.end = span.end;
            }
        }
    }

    pub fn fecha(&mut self) {
        self.abertos.pop();
    }

    pub fn fim(mut self) -> Cst {
        self.abertos.clear();
        std::mem::take(&mut self.cst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_arvore_devolve_o_arquivo_byte_a_byte() {
        let fonte = "a: number\n  b: any\n";
        let mut b = Builder::new("arquivo");
        b.token("ident", 0..1);
        b.token("punct", 1..2);
        b.token("trivia", 2..3);
        b.abre("tipo", 3);
        b.token("ident", 3..9);
        b.fecha();
        b.token("trivia", 9..12);
        b.token("ident", 12..13);
        b.token("punct", 13..14);
        b.token("trivia", 14..15);
        b.token("ident", 15..18);
        b.token("trivia", 18..19);
        let cst = b.fim();
        assert_eq!(cst.reimprime_byte_a_byte(fonte), fonte);
        assert_eq!(cst.texto(cst.raiz(), fonte), fonte);
    }
}
