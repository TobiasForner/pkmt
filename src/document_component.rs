use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DocumentElement {
    Heading(u16, String),
    /// file, optional section, optional rename
    FileLink(String, Option<String>, Option<String>),
    FileEmbed(String, Option<String>),
    Text(String),
    /// text, map storing additional properties
    Admonition(Vec<DocumentComponent>, HashMap<String, String>),
}

impl DocumentElement {
    fn to_logseq_text(&self) -> String {
        use DocumentElement::*;
        let mut tmp = self.clone();
        tmp.cleanup();
        match self {
            Heading(level, title) => {
                let title = title.trim();
                let hashes = "#".repeat(*level as usize).to_string();
                format!("- {hashes} {title}")
            }
            // todo use other parsed properties
            FileLink(file, _, _) => format!("[[{file}]]"),
            FileEmbed(file, _) => format!("{{{{embed [[{file}]]}}}}"),
            Text(text) => {
                if text.trim().is_empty() {
                    let line_count = text.lines().count();
                    if line_count >= 3 {
                        String::from("\n\n")
                    } else {
                        "\n".repeat(line_count).to_string()
                    }
                } else {
                    text.clone()
                }
            }
            Admonition(s, props) => {
                let mut parts = vec!["#+BEGIN_QUOTE".to_string()];
                if let Some(title) = props.get("title") {
                    parts.push(format!("**{title}**"));
                }
                let body = s
                    .iter()
                    .map(|c| c.to_logseq_text())
                    .collect::<Vec<String>>()
                    .join("");
                parts.push(body);
                parts.push("#+END_QUOTE".to_string());
                parts.join("\n")
            }
        }
    }

    fn cleanup(&mut self) {
        use DocumentElement::*;
        match self {
            Heading(_, text) => *text = text.trim().to_string(),
            FileLink(file, _, _) => *file = file.trim().to_string(),
            FileEmbed(file, _) => *file = file.trim().to_string(),
            Text(text) => {
                *text = DocumentElement::cleanup_text(&text);
            }
            Admonition(components, _) => {
                components.iter_mut().for_each(|c| c.element.cleanup());
            }
        }
    }

    fn cleanup_text(text: &str) -> String {
        let mut lines = vec![];
        let mut last_was_empty = false;
        text.trim().lines().for_each(|l| {
            if l.trim().is_empty() {
                last_was_empty = true;
            } else {
                if last_was_empty {
                    lines.push("");
                }
                lines.push(l);
            }
        });
        lines.join("\n")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentComponent {
    element: DocumentElement,
    children: Vec<Self>,
}

impl DocumentComponent {
    fn to_logseq_text(&self) -> String {
        [self.element.to_logseq_text()]
            .into_iter()
            .chain(self.children.iter().map(|c| {
                c.to_logseq_text()
                    .lines()
                    .map(|line| format!("\t{line}"))
                    .collect::<String>()
            }))
            .collect()
    }
    pub fn new(element: DocumentElement) -> Self {
        Self {
            element,
            children: vec![],
        }
    }

    pub fn new_text(text: &str) -> Self {
        Self::new(DocumentElement::Text(text.to_string()))
    }
}

pub fn to_logseq_text(components: &Vec<DocumentComponent>) -> String {
    components
        .iter()
        .map(|c| c.to_logseq_text())
        .collect::<Vec<String>>()
        .join("")
        .trim()
        .to_string()
}

pub fn collapse_text(components: &Vec<DocumentComponent>) -> Vec<DocumentComponent> {
    use DocumentElement::*;
    let mut text = String::new();
    let mut res: Vec<DocumentComponent> = vec![];
    components.iter().for_each(|c| match &c.element {
        Text(s) => {
            text.push_str(&s);
        }
        Admonition(components, properties) => {
            if !text.is_empty() {
                res.push(DocumentComponent::new_text(&text));
                text = String::new();
            }
            let collapsed = collapse_text(components);
            res.push(DocumentComponent::new(Admonition(
                collapsed,
                properties.clone(),
            )));
        }
        _ => {
            if !text.is_empty() {
                res.push(DocumentComponent::new_text(&text));
                text = String::new();
            }
            res.push(c.clone());
        }
    });
    if !text.is_empty() {
        res.push(DocumentComponent::new_text(&text));
    }
    res
}
