use std::fs;
use std::path::Path;

use byteorder::{ByteOrder, LittleEndian};
use thiserror::Error;

pub type Identifier = [u8; 4];

const WILDCARD_IDENTIFIER: Identifier = [0, 0, 0, 0];
const MINIMAL_FILE_SIZE: usize = 7;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Node {
    pub children: Vec<Node>,
    pub props_begin: usize,
    pub props_end: usize,
    pub node_type: u8,
}

impl Node {
    pub const ESCAPE: u8 = 0xFD;
    pub const START: u8 = 0xFE;
    pub const END: u8 = 0xFF;
}

#[derive(Debug, Error)]
pub enum OtbError {
    #[error("failed to read `{path}`: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid OTB file format")]
    InvalidFormat,
    #[error("unexpected end of OTB property stream")]
    UnexpectedEof,
}

#[derive(Debug, Clone)]
pub struct Loader {
    file_contents: Vec<u8>,
    root: Node,
}

impl Loader {
    pub fn from_path(
        path: impl AsRef<Path>,
        accepted_identifier: Identifier,
    ) -> Result<Self, OtbError> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|source| OtbError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_bytes(bytes, accepted_identifier)
    }

    pub fn from_bytes(
        file_contents: Vec<u8>,
        accepted_identifier: Identifier,
    ) -> Result<Self, OtbError> {
        if file_contents.len() <= MINIMAL_FILE_SIZE {
            return Err(OtbError::InvalidFormat);
        }

        let file_identifier: Identifier = file_contents[..accepted_identifier.len()]
            .try_into()
            .map_err(|_| OtbError::InvalidFormat)?;

        if file_identifier != accepted_identifier && file_identifier != WILDCARD_IDENTIFIER {
            return Err(OtbError::InvalidFormat);
        }

        Ok(Self {
            file_contents,
            root: Node::default(),
        })
    }

    pub fn parse_tree(&mut self) -> Result<&Node, OtbError> {
        let mut index = std::mem::size_of::<Identifier>();
        if self.file_contents.get(index).copied() != Some(Node::START) {
            return Err(OtbError::InvalidFormat);
        }

        let Some(root_type) = self.file_contents.get(index + 1).copied() else {
            return Err(OtbError::InvalidFormat);
        };

        self.root = Node {
            children: Vec::new(),
            props_begin: index + 2,
            props_end: 0,
            node_type: root_type,
        };

        let mut parse_stack = vec![Vec::new()];
        index += 2;

        while index < self.file_contents.len() {
            match self.file_contents[index] {
                Node::START => {
                    let current_path =
                        parse_stack.last().cloned().ok_or(OtbError::InvalidFormat)?;
                    let current_node = get_node_mut(&mut self.root, &current_path)?;
                    if current_node.children.is_empty() {
                        current_node.props_end = index;
                    }

                    index += 1;
                    let Some(node_type) = self.file_contents.get(index).copied() else {
                        return Err(OtbError::InvalidFormat);
                    };

                    current_node.children.push(Node {
                        children: Vec::new(),
                        props_begin: index + 1,
                        props_end: 0,
                        node_type,
                    });

                    let mut child_path = current_path;
                    child_path.push(current_node.children.len() - 1);
                    parse_stack.push(child_path);
                }
                Node::END => {
                    let current_path = parse_stack.pop().ok_or(OtbError::InvalidFormat)?;
                    let current_node = get_node_mut(&mut self.root, &current_path)?;
                    if current_node.children.is_empty() {
                        current_node.props_end = index;
                    }
                }
                Node::ESCAPE => {
                    index += 1;
                    if index == self.file_contents.len() {
                        return Err(OtbError::InvalidFormat);
                    }
                }
                _ => {}
            }

            index += 1;
        }

        if !parse_stack.is_empty() {
            return Err(OtbError::InvalidFormat);
        }

        Ok(&self.root)
    }

    pub fn get_props(&self, node: &Node) -> Result<Option<PropStream>, OtbError> {
        if node.props_end < node.props_begin || node.props_end > self.file_contents.len() {
            return Err(OtbError::InvalidFormat);
        }

        let size = node.props_end - node.props_begin;
        if size == 0 {
            return Ok(None);
        }

        let mut prop_buffer = Vec::with_capacity(size);
        let mut last_escaped = false;

        for byte in &self.file_contents[node.props_begin..node.props_end] {
            last_escaped = *byte == Node::ESCAPE && !last_escaped;
            if !last_escaped {
                prop_buffer.push(*byte);
            }
        }

        Ok(Some(PropStream::new(prop_buffer)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PropStream {
    buffer: Vec<u8>,
    position: usize,
}

impl PropStream {
    pub fn new(buffer: Vec<u8>) -> Self {
        Self {
            buffer,
            position: 0,
        }
    }

    pub fn remaining(&self) -> usize {
        self.buffer.len().saturating_sub(self.position)
    }

    pub fn read_u8(&mut self) -> Result<u8, OtbError> {
        Ok(self.read_exact::<1>()?[0])
    }

    pub fn read_optional_u8(&mut self) -> Result<Option<u8>, OtbError> {
        if self.remaining() == 0 {
            return Ok(None);
        }

        self.read_u8().map(Some)
    }

    pub fn read_u16(&mut self) -> Result<u16, OtbError> {
        Ok(LittleEndian::read_u16(&self.read_exact::<2>()?))
    }

    pub fn read_u32(&mut self) -> Result<u32, OtbError> {
        Ok(LittleEndian::read_u32(&self.read_exact::<4>()?))
    }

    pub fn read_string(&mut self) -> Result<Vec<u8>, OtbError> {
        let string_len = usize::from(self.read_u16()?);
        Ok(self.read_bytes(string_len)?.to_vec())
    }

    pub fn read_fixed_bytes<const N: usize>(&mut self) -> Result<[u8; N], OtbError> {
        self.read_exact::<N>()
    }

    pub fn read_bytes(&mut self, size: usize) -> Result<&[u8], OtbError> {
        if self.remaining() < size {
            return Err(OtbError::UnexpectedEof);
        }

        let start = self.position;
        let end = start + size;
        self.position = end;
        Ok(&self.buffer[start..end])
    }

    pub fn skip(&mut self, count: usize) -> Result<(), OtbError> {
        let _ = self.read_bytes(count)?;
        Ok(())
    }

    fn read_exact<const N: usize>(&mut self) -> Result<[u8; N], OtbError> {
        let bytes = self.read_bytes(N)?;
        bytes.try_into().map_err(|_| OtbError::UnexpectedEof)
    }
}

fn get_node_mut<'a>(root: &'a mut Node, path: &[usize]) -> Result<&'a mut Node, OtbError> {
    let mut current = root;
    for &index in path {
        current = current
            .children
            .get_mut(index)
            .ok_or(OtbError::InvalidFormat)?;
    }
    Ok(current)
}

#[cfg(test)]
mod tests {
    use super::{Loader, Node};

    #[test]
    fn parse_tree_should_preserve_nested_nodes_and_unescape_properties() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"OTBI");
        bytes.extend_from_slice(&[
            Node::START,
            0x01,
            0xAA,
            Node::ESCAPE,
            Node::START,
            0xBB,
            Node::START,
            0x02,
            0x10,
            Node::ESCAPE,
            Node::END,
            0x20,
            Node::END,
            Node::END,
        ]);

        let mut loader = Loader::from_bytes(bytes, *b"OTBI").expect("identifier should match");
        let root = loader.parse_tree().expect("tree should parse").clone();

        assert_eq!(root.node_type, 0x01);
        assert_eq!(root.children.len(), 1);

        let root_props = loader
            .get_props(&root)
            .expect("props should decode")
            .expect("root should have props");
        assert_eq!(
            root_props,
            super::PropStream::new(vec![0xAA, Node::START, 0xBB])
        );

        let child = &root.children[0];
        assert_eq!(child.node_type, 0x02);
        let child_props = loader
            .get_props(child)
            .expect("child props should decode")
            .expect("child should have props");
        assert_eq!(
            child_props,
            super::PropStream::new(vec![0x10, Node::END, 0x20])
        );
    }

    #[test]
    fn parse_tree_should_reject_invalid_identifier() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"ABCD");
        bytes.extend_from_slice(&[Node::START, 0x01, Node::END]);

        let error = Loader::from_bytes(bytes, *b"OTBI").expect_err("identifier should fail");
        assert_eq!(error.to_string(), "invalid OTB file format");
    }

    #[test]
    fn prop_stream_should_read_little_endian_values_and_length_prefixed_strings() {
        let mut stream = super::PropStream::new(vec![
            0x34, 0x12, 0x78, 0x56, 0x34, 0x12, 0x03, 0x00, b'a', b'b', b'c',
        ]);

        assert_eq!(stream.read_u16().expect("u16 should read"), 0x1234);
        assert_eq!(stream.read_u32().expect("u32 should read"), 0x1234_5678);
        assert_eq!(stream.read_string().expect("string should read"), b"abc");
    }

    #[test]
    fn parse_tree_should_allow_wildcard_identifier() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&[Node::START, 0x01, 0xAA, Node::END]);

        let mut loader = Loader::from_bytes(bytes, *b"OTBI").expect("wildcard should pass");
        let root = loader.parse_tree().expect("tree should parse").clone();

        assert_eq!(root.node_type, 0x01);
    }
}
