use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentElement {
    Heading(u16, String),
    /// file, optional section, optional rename
    FileLink(String, Option<String>, Option<String>),
    FileEmbed(String, Option<String>),
    Text(String),
    /// text, map storing additional properties
    Admonition(String, HashMap<String, String>),
}

impl DocumentElement {
    fn is_inline_text(&self) -> bool {
        match self {
            DocumentElement::Text(_) => true,
            _ => false,
        }
    }

    fn obsidian_to_logseq(&self) -> Self {
        use DocumentElement::*;
        match self {
            Admonition(s, props) => {
                todo!("convert to logseq")
            }
            _ => todo!("handle other element types"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentComponent {
    element: DocumentElement,
    children: Vec<Self>,
}

impl DocumentComponent {
    pub fn new(element: DocumentElement) -> Self {
        Self {
            element,
            children: vec![],
        }
    }

    pub fn new_text(text: &str) -> Self {
        Self::new(DocumentElement::Text(text.to_string()))
    }

    fn is_inline_text(&self) -> bool {
        if self.element.is_inline_text() {
            assert!(self.children.is_empty(), "");
            true
        } else {
            false
        }
    }
}

pub fn collapse_text(components: &Vec<DocumentComponent>) -> Vec<DocumentComponent> {
    let mut text = String::new();
    let mut res: Vec<DocumentComponent> = vec![];
    components.iter().for_each(|c| {
        if c.is_inline_text() {
            match &c.element {
                DocumentElement::Text(s) => {
                    text.push_str(&s);
                }
                _ => panic!("{c:?} is not text!"),
            }
        } else {
            if !text.is_empty() {
                res.push(DocumentComponent::new_text(&text));
                text = String::new();
            }
            res.push(c.clone());
        }
    });
    res
}
