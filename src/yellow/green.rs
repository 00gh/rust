use std::sync::Arc;
use {
    smol_str::SmolStr,
    SyntaxKind::{self, *},
    TextUnit,
};

#[derive(Clone, Debug)]
pub(crate) enum GreenNode {
    Leaf(GreenLeaf),
    Branch(Arc<GreenBranch>),
}

impl GreenNode {
    pub(crate) fn new_leaf(kind: SyntaxKind, text: &str) -> GreenNode {
        GreenNode::Leaf(GreenLeaf::new(kind, text))
    }

    pub(crate) fn new_branch(kind: SyntaxKind, children: Vec<GreenNode>) -> GreenNode {
        GreenNode::Branch(Arc::new(GreenBranch::new(kind, children)))
    }

    pub fn kind(&self) -> SyntaxKind {
        match self {
            GreenNode::Leaf(l) => l.kind(),
            GreenNode::Branch(b) => b.kind(),
        }
    }

    pub fn text_len(&self) -> TextUnit {
        match self {
            GreenNode::Leaf(l) => l.text_len(),
            GreenNode::Branch(b) => b.text_len(),
        }
    }

    pub fn children(&self) -> &[GreenNode] {
        match self {
            GreenNode::Leaf(_) => &[],
            GreenNode::Branch(b) => b.children(),
        }
    }

    pub fn text(&self) -> String {
        let mut buff = String::new();
        go(self, &mut buff);
        return buff;
        fn go(node: &GreenNode, buff: &mut String) {
            match node {
                GreenNode::Leaf(l) => buff.push_str(&l.text()),
                GreenNode::Branch(b) => b.children().iter().for_each(|child| go(child, buff)),
            }
        }
    }
}

#[test]
fn assert_send_sync() {
    fn f<T: Send + Sync>() {}
    f::<GreenNode>();
}

#[derive(Clone, Debug)]
pub(crate) struct GreenBranch {
    text_len: TextUnit,
    kind: SyntaxKind,
    children: Vec<GreenNode>,
}

impl GreenBranch {
    fn new(kind: SyntaxKind, children: Vec<GreenNode>) -> GreenBranch {
        let text_len = children.iter().map(|x| x.text_len()).sum::<TextUnit>();
        GreenBranch {
            text_len,
            kind,
            children,
        }
    }

    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }

    pub fn text_len(&self) -> TextUnit {
        self.text_len
    }

    pub fn children(&self) -> &[GreenNode] {
        self.children.as_slice()
    }
}

#[derive(Clone, Debug)]
pub(crate) enum GreenLeaf {
    Whitespace {
        newlines: u8,
        spaces: u8,
    },
    Token {
        kind: SyntaxKind,
        text: Option<SmolStr>,
    },
}

impl GreenLeaf {
    fn new(kind: SyntaxKind, text: &str) -> Self {
        if kind == WHITESPACE {
            let newlines = text.bytes().take_while(|&b| b == b'\n').count();
            let spaces = text[newlines..].bytes().take_while(|&b| b == b' ').count();
            if newlines + spaces == text.len() && newlines <= N_NEWLINES && spaces <= N_SPACES {
                return GreenLeaf::Whitespace {
                    newlines: newlines as u8,
                    spaces: spaces as u8,
                };
            }
        }
        let text = match SyntaxKind::static_text(kind) {
            Some(t) => {
                debug_assert_eq!(t, text);
                None
            }
            None => Some(SmolStr::new(text)),
        };
        GreenLeaf::Token { kind, text }
    }

    pub(crate) fn kind(&self) -> SyntaxKind {
        match self {
            GreenLeaf::Whitespace { .. } => WHITESPACE,
            GreenLeaf::Token { kind, .. } => *kind,
        }
    }

    pub(crate) fn text(&self) -> &str {
        match self {
            &GreenLeaf::Whitespace { newlines, spaces } => {
                let newlines = newlines as usize;
                let spaces = spaces as usize;
                assert!(newlines <= N_NEWLINES && spaces <= N_SPACES);
                &WS[N_NEWLINES - newlines..N_NEWLINES + spaces]
            }
            GreenLeaf::Token { kind, text } => match text {
                None => kind.static_text().unwrap(),
                Some(t) => t.as_str(),
            },
        }
    }

    pub(crate) fn text_len(&self) -> TextUnit {
        TextUnit::of_str(self.text())
    }
}

const N_NEWLINES: usize = 16;
const N_SPACES: usize = 64;
const WS: &str =
    "\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n\n                                                                ";
