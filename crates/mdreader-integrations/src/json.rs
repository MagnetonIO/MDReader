//! Lossless JSON edits: unchanged tokens are never serialized again.
use crate::{IntegrationError as Error, Result};
use serde_json::Value;
use std::{collections::HashSet, ops::Range};

#[derive(Debug)]
struct Node {
    range: Range<usize>,
    kind: Kind,
}
#[derive(Debug)]
enum Kind {
    Object(Vec<Member>),
    Array(Vec<Node>),
    Scalar,
}
#[derive(Debug)]
struct Member {
    key: String,
    start: usize,
    value: Node,
}
struct Parser<'a> {
    input: &'a str,
    at: usize,
}
impl Parser<'_> {
    fn whitespace(&mut self) {
        while self
            .input
            .as_bytes()
            .get(self.at)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.at += 1;
        }
    }
    fn string(&mut self) -> Result<String> {
        let start = self.at;
        if self.input.as_bytes().get(self.at) != Some(&b'"') {
            return Err(Error::Invalid);
        }
        self.at += 1;
        loop {
            match self.input.as_bytes().get(self.at) {
                Some(b'"') => {
                    self.at += 1;
                    break;
                }
                Some(b'\\') => {
                    self.at += 2;
                }
                Some(_) => self.at += 1,
                None => return Err(Error::Invalid),
            }
        }
        serde_json::from_str(&self.input[start..self.at]).map_err(|_| Error::Invalid)
    }
    fn value(&mut self, depth: usize) -> Result<Node> {
        if depth > 64 {
            return Err(Error::Bounds);
        }
        self.whitespace();
        let start = self.at;
        let kind = match self.input.as_bytes().get(self.at) {
            Some(b'{') => {
                self.at += 1;
                self.whitespace();
                let mut members = Vec::new();
                let mut keys = HashSet::new();
                if self.input.as_bytes().get(self.at) != Some(&b'}') {
                    loop {
                        self.whitespace();
                        let member_start = self.at;
                        let key = self.string()?;
                        if !keys.insert(key.clone()) {
                            return Err(Error::Invalid);
                        }
                        self.whitespace();
                        if self.input.as_bytes().get(self.at) != Some(&b':') {
                            return Err(Error::Invalid);
                        }
                        self.at += 1;
                        let value = self.value(depth + 1)?;
                        members.push(Member {
                            key,
                            start: member_start,
                            value,
                        });
                        self.whitespace();
                        match self.input.as_bytes().get(self.at) {
                            Some(b',') => self.at += 1,
                            Some(b'}') => break,
                            _ => return Err(Error::Invalid),
                        }
                    }
                }
                self.at += 1;
                Kind::Object(members)
            }
            Some(b'[') => {
                self.at += 1;
                self.whitespace();
                let mut values = Vec::new();
                if self.input.as_bytes().get(self.at) != Some(&b']') {
                    loop {
                        values.push(self.value(depth + 1)?);
                        self.whitespace();
                        match self.input.as_bytes().get(self.at) {
                            Some(b',') => self.at += 1,
                            Some(b']') => break,
                            _ => return Err(Error::Invalid),
                        }
                    }
                }
                self.at += 1;
                Kind::Array(values)
            }
            Some(b'"') => {
                self.string()?;
                Kind::Scalar
            }
            Some(_) => {
                while self
                    .input
                    .as_bytes()
                    .get(self.at)
                    .is_some_and(|b| !b.is_ascii_whitespace() && !b",]}".contains(b))
                {
                    self.at += 1;
                }
                if self.at == start {
                    return Err(Error::Invalid);
                }
                serde_json::from_str::<Value>(&self.input[start..self.at])
                    .map_err(|_| Error::Invalid)?;
                Kind::Scalar
            }
            None => return Err(Error::Invalid),
        };
        Ok(Node {
            range: start..self.at,
            kind,
        })
    }
}
fn parse(input: &str) -> Result<Node> {
    let mut parser = Parser { input, at: 0 };
    let node = parser.value(0)?;
    parser.whitespace();
    if parser.at != input.len() || !matches!(node.kind, Kind::Object(_)) {
        return Err(Error::Invalid);
    }
    Ok(node)
}
pub(crate) fn validate(input: &str) -> Result<()> {
    parse(input).map(|_| ())
}
fn object(node: &Node) -> Result<&[Member]> {
    if let Kind::Object(m) = &node.kind {
        Ok(m)
    } else {
        Err(Error::Invalid)
    }
}
fn child<'a>(node: &'a Node, key: &str) -> Result<Option<&'a Node>> {
    Ok(object(node)?
        .iter()
        .find(|m| m.key == key)
        .map(|m| &m.value))
}
fn locate<'a>(node: &'a Node, path: &[String]) -> Result<Option<&'a Node>> {
    let mut found = node;
    for key in path {
        match child(found, key)? {
            Some(next) => found = next,
            None => return Ok(None),
        }
    }
    Ok(Some(found))
}
fn insert_at(input: &str, at: usize, text: &str) -> String {
    let mut out = input.to_owned();
    out.insert_str(at, text);
    out
}
fn add_member(input: &str, node: &Node, key: &str, raw: &str) -> Result<String> {
    let members = object(node)?;
    let key = serde_json::to_string(key).map_err(|_| Error::Invalid)?;
    Ok(insert_at(
        input,
        node.range.end - 1,
        &format!("{}{key}:{raw}", if members.is_empty() { "" } else { "," }),
    ))
}
pub(crate) fn install(input: &str, path: &[String], array: bool, raw: &str) -> Result<String> {
    let root = parse(input)?;
    if path.is_empty() {
        return Err(Error::Invalid);
    }
    let expected: Value = serde_json::from_str(raw).map_err(|_| Error::Invalid)?;
    let mut parent = &root;
    for (index, key) in path.iter().enumerate() {
        let existing = child(parent, key)?;
        if index == path.len() - 1 {
            return if array {
                match existing {
                    None => add_member(input, parent, key, &format!("[{raw}]")),
                    Some(node) => {
                        let Kind::Array(values) = &node.kind else {
                            return Err(Error::Invalid);
                        };
                        for value in values {
                            if serde_json::from_str::<Value>(&input[value.range.clone()])
                                .map_err(|_| Error::Invalid)?
                                == expected
                            {
                                return Err(Error::Conflict);
                            }
                        }
                        Ok(insert_at(
                            input,
                            node.range.end - 1,
                            &format!("{}{raw}", if values.is_empty() { "" } else { "," }),
                        ))
                    }
                }
            } else {
                if existing.is_some() {
                    return Err(Error::Conflict);
                }
                add_member(input, parent, key, raw)
            };
        }
        match existing {
            Some(node) => parent = node,
            None => {
                let mut nested = if array {
                    format!("[{raw}]")
                } else {
                    raw.to_owned()
                };
                for key in path[index + 1..].iter().rev() {
                    nested = format!(
                        "{}:{}",
                        serde_json::to_string(key).map_err(|_| Error::Invalid)?,
                        nested
                    );
                    nested = format!("{{{nested}}}");
                }
                return add_member(input, parent, key, &nested);
            }
        }
    }
    Err(Error::Invalid)
}
pub(crate) fn verify(input: &str, path: &[String], array: bool, raw: &str) -> Result<bool> {
    let root = parse(input)?;
    let Some(node) = locate(&root, path)? else {
        return Ok(false);
    };
    if !array {
        return if &input[node.range.clone()] == raw {
            Ok(true)
        } else {
            Err(Error::Conflict)
        };
    }
    let Kind::Array(values) = &node.kind else {
        return Err(Error::Conflict);
    };
    let count = values
        .iter()
        .filter(|node| &input[node.range.clone()] == raw)
        .count();
    if count == 1 {
        Ok(true)
    } else {
        Err(Error::Conflict)
    }
}
pub(crate) fn remove(input: &str, path: &[String], array: bool, raw: &str) -> Result<String> {
    if !verify(input, path, array, raw)? {
        return Ok(input.to_owned());
    }
    let root = parse(input)?;
    let range = if array {
        let node = locate(&root, path)?.ok_or(Error::Conflict)?;
        let Kind::Array(values) = &node.kind else {
            return Err(Error::Conflict);
        };
        let index = values
            .iter()
            .position(|node| &input[node.range.clone()] == raw)
            .ok_or(Error::Conflict)?;
        if values.len() == 1 {
            values[0].range.clone()
        } else if index > 0 {
            values[index - 1].range.end..values[index].range.end
        } else {
            values[0].range.start..values[1].range.start
        }
    } else {
        let parent = locate(&root, &path[..path.len() - 1])?.ok_or(Error::Conflict)?;
        let members = object(parent)?;
        let index = members
            .iter()
            .position(|m| m.key == path[path.len() - 1])
            .ok_or(Error::Conflict)?;
        if members.len() == 1 {
            members[0].start..members[0].value.range.end
        } else if index > 0 {
            members[index - 1].value.range.end..members[index].value.range.end
        } else {
            members[0].start..members[1].start
        }
    };
    let mut output = input.to_owned();
    output.replace_range(range, "");
    validate(&output)?;
    Ok(output)
}
