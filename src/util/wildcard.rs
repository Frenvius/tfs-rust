use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReturnValue {
    NoError,
    PlayerWithThisNameIsNotOnline,
    NameIsTooAmbiguous,
}

#[derive(Debug, Default)]
pub struct WildcardTreeNode {
    children: BTreeMap<char, WildcardTreeNode>,
    breakpoint: bool,
}

impl WildcardTreeNode {
    pub fn new(breakpoint: bool) -> Self {
        Self {
            children: BTreeMap::new(),
            breakpoint,
        }
    }

    pub fn insert(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }

        let mut current = self;
        let mut chars = value.chars().peekable();

        while let Some(ch) = chars.next() {
            let breakpoint = chars.peek().is_none();
            current = current.add_child(ch, breakpoint);
        }
    }

    pub fn remove(&mut self, value: &str) {
        let chars: Vec<char> = value.chars().collect();
        self.remove_internal(&chars, 0);
    }

    pub fn find_one(&self, query: &str) -> (ReturnValue, String) {
        let mut current = self;
        for ch in query.chars() {
            let Some(child) = current.get_child(ch) else {
                return (ReturnValue::PlayerWithThisNameIsNotOnline, String::new());
            };
            current = child;
        }

        let mut result = String::from(query);
        loop {
            let size = current.children.len();
            if size == 0 {
                return (ReturnValue::NoError, result);
            }

            if size > 1 || current.breakpoint {
                return (ReturnValue::NameIsTooAmbiguous, result);
            }

            let (ch, child) = current
                .children
                .iter()
                .next()
                .expect("single-child branch must exist");
            result.push(*ch);
            current = child;
        }
    }

    fn get_child(&self, ch: char) -> Option<&WildcardTreeNode> {
        self.children.get(&ch)
    }

    fn add_child(&mut self, ch: char, breakpoint: bool) -> &mut WildcardTreeNode {
        let child = self
            .children
            .entry(ch)
            .or_insert_with(|| WildcardTreeNode::new(breakpoint));
        if breakpoint && !child.breakpoint {
            child.breakpoint = true;
        }
        child
    }

    fn remove_internal(&mut self, chars: &[char], index: usize) -> bool {
        if index == chars.len() {
            self.breakpoint = false;
        } else {
            let should_remove_child = {
                let Some(child) = self.children.get_mut(&chars[index]) else {
                    return false;
                };
                child.remove_internal(chars, index + 1)
            };

            if should_remove_child {
                self.children.remove(&chars[index]);
            }
        }

        self.children.is_empty() && !self.breakpoint
    }
}

#[cfg(test)]
mod tests {
    use super::{ReturnValue, WildcardTreeNode};

    #[test]
    fn find_one_should_complete_a_unique_prefix() {
        let mut tree = WildcardTreeNode::default();
        tree.insert("alice");

        let (result, value) = tree.find_one("ali");

        assert_eq!(result, ReturnValue::NoError);
        assert_eq!(value, "alice");
    }

    #[test]
    fn find_one_should_report_ambiguity_when_multiple_names_match() {
        let mut tree = WildcardTreeNode::default();
        tree.insert("alice");
        tree.insert("alina");

        let (result, value) = tree.find_one("ali");

        assert_eq!(result, ReturnValue::NameIsTooAmbiguous);
        assert_eq!(value, "ali");
    }

    #[test]
    fn remove_should_prune_unused_nodes() {
        let mut tree = WildcardTreeNode::default();
        tree.insert("alice");
        tree.remove("alice");

        let (result, _) = tree.find_one("ali");

        assert_eq!(result, ReturnValue::PlayerWithThisNameIsNotOnline);
    }
}
