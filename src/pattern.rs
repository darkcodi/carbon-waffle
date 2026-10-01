//! Finite password patterns. Only the source words, syntax tree, and current
//! candidate are kept in memory; Cartesian products are never materialized.
use regex_syntax::hir::{Class, Hir, HirKind};
use std::{
    fs::File,
    io::{self, BufRead, BufReader, Read},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

const MAX_LENGTH: usize = 63;
const MEMORY_LIMIT: usize = 256 * 1024 * 1024;
type Lengths = [u128; MAX_LENGTH + 1];
type Words = [Vec<Vec<u8>>; 4];

#[derive(Debug, Clone)]
enum Expr {
    Literal(Vec<u8>),
    Class(Vec<u8>),
    Word(usize),
    Concat(Vec<Expr>),
    Either(Vec<Expr>),
    Repeat(Box<Expr>, u32, u32),
}

#[derive(Debug, Clone)]
pub struct Pattern {
    root: Expr,
    cases: [bool; 4],
}

#[derive(Debug, Clone)]
pub struct Preview {
    pub total: u128,
    pub words: usize,
    pub samples: Vec<String>,
}

pub struct Prepared {
    root: Node,
    words: Words,
    pub total: u128,
    pub word_count: usize,
}

struct Node {
    kind: Kind,
    lengths: Lengths,
    min: usize,
    max: usize,
}

enum Kind {
    Literal(Vec<u8>),
    Class(Vec<u8>),
    Word(usize),
    Concat(Vec<Node>),
    Either(Vec<Node>),
    Repeat(Box<Node>, u32, u32),
}

impl Pattern {
    pub fn parse(source: &str) -> Result<Self, String> {
        if source.is_empty() || source.len() > 512 {
            return Err("Enter a pattern of 1–512 bytes.".into());
        }
        if source.contains("__cw_word_") {
            return Err("The name __cw_word_ is reserved for word placeholders.".into());
        }
        let mut input = source.strip_prefix('^').unwrap_or(source);
        // Outer anchors are optional: candidates always match the whole pattern.
        if input.ends_with('$')
            && input
                .as_bytes()
                .iter()
                .rev()
                .skip(1)
                .take_while(|&&b| b == b'\\')
                .count()
                % 2
                == 0
        {
            input = &input[..input.len() - 1];
        }
        let mut expanded = String::new();
        let mut placeholders = Vec::new();
        let mut class_depth = 0usize;
        let mut cases = [false; 4];
        while !input.is_empty() {
            let ch = input.chars().next().unwrap();
            if ch == '\\' {
                expanded.push(ch);
                input = &input[1..];
                if let Some(next) = input.chars().next() {
                    expanded.push(next);
                    input = &input[next.len_utf8()..];
                }
                continue;
            }
            if class_depth == 0
                && let Some((case, token)) = ["{word}", "{Word}", "{WORD}", "{lower}"]
                    .into_iter()
                    .enumerate()
                    .find(|(_, token)| input.starts_with(token))
            {
                expanded.push_str(&format!("(?P<__cw_word_{}>x)", placeholders.len()));
                placeholders.push(case);
                cases[case] = true;
                input = &input[token.len()..];
                continue;
            }
            if ch == '[' {
                class_depth += 1;
            }
            if ch == ']' {
                class_depth = class_depth.saturating_sub(1);
            }
            expanded.push(ch);
            input = &input[ch.len_utf8()..];
        }
        let hir = regex_syntax::ParserBuilder::new()
            .unicode(false)
            .utf8(false)
            .nest_limit(32)
            .build()
            .parse(&expanded)
            .map_err(|error| {
                let message = error.to_string();
                format!(
                    "Invalid pattern: {}",
                    message.lines().last().unwrap_or(&message)
                )
            })?;
        let root = expression(&hir, &placeholders)?;
        if expanded_items(&root) > 512 {
            return Err("Nested repeats expand to too many items. Simplify the pattern.".into());
        }
        Ok(Self { root, cases })
    }

    pub fn needs_words(&self) -> bool {
        self.cases.iter().any(|used| *used)
    }

    pub fn prepare(&self, path: &Path, cancel: &AtomicBool) -> Result<Prepared, String> {
        let mut words: Words = std::array::from_fn(|_| Vec::new());
        let mut word_count = 0;
        let mut memory = 0;
        if self.needs_words() {
            let file = File::options()
                .read(true)
                .custom_flags(nix::libc::O_NONBLOCK)
                .open(path)
                .map_err(|e| format!("Cannot read wordlist: {e}"))?;
            let metadata = file.metadata().map_err(|e| e.to_string())?;
            if !metadata.is_file() || metadata.len() > MEMORY_LIMIT as u64 {
                return Err(
                    "Choose a regular wordlist file smaller than 256 MiB for pattern recovery."
                        .into(),
                );
            }
            let mut reader = BufReader::new(file.take(MEMORY_LIMIT as u64 + 1));
            let mut read = 0;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err("Pattern preparation cancelled.".into());
                }
                let mut word = Vec::new();
                let size = reader
                    .read_until(b'\n', &mut word)
                    .map_err(|e| e.to_string())?;
                if size == 0 {
                    break;
                }
                read += size;
                if read > MEMORY_LIMIT {
                    return Err("The wordlist grew beyond the 256 MiB limit.".into());
                }
                if word.last() == Some(&b'\n') {
                    word.pop();
                }
                if word.last() == Some(&b'\r') {
                    word.pop();
                }
                if word.is_empty()
                    || word.len() > MAX_LENGTH
                    || word.contains(&0)
                    || word.contains(&b'\r')
                {
                    continue;
                }
                word_count += 1;
                for (case, used) in self.cases.iter().enumerate() {
                    if !used {
                        continue;
                    }
                    memory += word.len() + std::mem::size_of::<Vec<u8>>();
                    if memory > MEMORY_LIMIT {
                        return Err("Pattern source words exceed 256 MiB in memory. Use a smaller wordlist.".into());
                    }
                    let mut value = word.clone();
                    match case {
                        1 => {
                            value.make_ascii_lowercase();
                            value[0].make_ascii_uppercase();
                        }
                        2 => value.make_ascii_uppercase(),
                        3 => value.make_ascii_lowercase(),
                        _ => {}
                    }
                    words[case].push(value);
                }
            }
            if word_count == 0 {
                return Err("The wordlist has no usable entries (1–63 bytes per word).".into());
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("Pattern preparation cancelled.".into());
        }
        let root = prepare_node(&self.root, &words);
        let total = root.lengths[8..]
            .iter()
            .fold(0u128, |sum, value| sum.saturating_add(*value));
        if total == 0 {
            return Err(
                "This pattern and wordlist cannot produce a WPA passphrase of 8–63 bytes.".into(),
            );
        }
        Ok(Prepared {
            root,
            words,
            total,
            word_count,
        })
    }
}

// Also bound traversal depth when nested repeats contain empty alternatives.
// Password length alone does not bound the number of zero-length items.
fn expanded_items(expr: &Expr) -> usize {
    match expr {
        Expr::Literal(_) | Expr::Class(_) | Expr::Word(_) => 1,
        Expr::Concat(parts) => parts
            .iter()
            .fold(1usize, |sum, part| sum.saturating_add(expanded_items(part))),
        Expr::Either(parts) => {
            1usize.saturating_add(parts.iter().map(expanded_items).max().unwrap_or(0))
        }
        Expr::Repeat(part, _, max) => {
            1usize.saturating_add(expanded_items(part).saturating_mul(*max as usize))
        }
    }
}

fn expression(hir: &Hir, placeholders: &[usize]) -> Result<Expr, String> {
    Ok(match hir.kind() {
        HirKind::Empty => Expr::Literal(Vec::new()),
        HirKind::Literal(literal) => {
            if literal.0.iter().any(|byte| matches!(byte, 0 | b'\n' | b'\r')) {
                return Err("Passwords cannot contain NUL or line breaks.".into());
            }
            Expr::Literal(literal.0.to_vec())
        }
        HirKind::Class(class) => {
            let values: Vec<u8> = (b' '..=b'~').filter(|byte| match class {
                Class::Bytes(class) => class.ranges().iter().any(|range| range.start() <= *byte && *byte <= range.end()),
                Class::Unicode(class) => class.ranges().iter().any(|range| range.start() <= char::from(*byte) && char::from(*byte) <= range.end()),
            }).collect();
            if values.is_empty() { return Err("Character classes must include printable ASCII characters.".into()); }
            Expr::Class(values)
        }
        HirKind::Capture(capture) => {
            if let Some(index) = capture.name.as_deref().and_then(|name| name.strip_prefix("__cw_word_")).and_then(|index| index.parse::<usize>().ok()) {
                if !matches!(capture.sub.kind(), HirKind::Literal(value) if value.0.as_ref() == b"x") {
                    return Err("Use {word}, {Word}, {WORD}, or {lower} for word casing; case-insensitive flags around word placeholders are not supported.".into());
                }
                Expr::Word(placeholders[index])
            } else {
                expression(&capture.sub, placeholders)?
            }
        }
        HirKind::Concat(parts) => Expr::Concat(parts.iter().map(|part| expression(part, placeholders)).collect::<Result<_, _>>()?),
        HirKind::Alternation(parts) => Expr::Either(parts.iter().map(|part| expression(part, placeholders)).collect::<Result<_, _>>()?),
        HirKind::Repetition(repeat) => {
            let max = repeat.max.ok_or("Use bounded repeats such as {3} or {1,5}; * and + are not supported.")?;
            if max > 63 { return Err("A repetition can contain at most 63 items.".into()); }
            Expr::Repeat(Box::new(expression(&repeat.sub, placeholders)?), repeat.min, max)
        }
        HirKind::Look(_) => return Err("Lookarounds and internal anchors are not supported; candidates match the whole pattern.".into()),
    })
}

fn product(a: &Lengths, b: &Lengths) -> Lengths {
    let mut result = [0u128; MAX_LENGTH + 1];
    for (i, left) in a.iter().enumerate().filter(|(_, count)| **count != 0) {
        for (j, right) in b
            .iter()
            .enumerate()
            .take(MAX_LENGTH + 1 - i)
            .filter(|(_, count)| **count != 0)
        {
            result[i + j] = result[i + j].saturating_add(left.saturating_mul(*right));
        }
    }
    result
}

fn prepare_node(expr: &Expr, words: &Words) -> Node {
    let mut lengths = [0u128; MAX_LENGTH + 1];
    let kind = match expr {
        Expr::Literal(value) => {
            if value.len() <= MAX_LENGTH {
                lengths[value.len()] = 1;
            }
            Kind::Literal(value.clone())
        }
        Expr::Class(values) => {
            lengths[1] = values.len() as u128;
            Kind::Class(values.clone())
        }
        Expr::Word(case) => {
            for value in &words[*case] {
                lengths[value.len()] += 1;
            }
            Kind::Word(*case)
        }
        Expr::Concat(parts) => {
            lengths[0] = 1;
            let nodes: Vec<_> = parts.iter().map(|part| prepare_node(part, words)).collect();
            for node in &nodes {
                lengths = product(&lengths, &node.lengths);
            }
            Kind::Concat(nodes)
        }
        Expr::Either(parts) => {
            let nodes: Vec<_> = parts.iter().map(|part| prepare_node(part, words)).collect();
            for node in &nodes {
                for (length, count) in lengths.iter_mut().zip(node.lengths) {
                    *length = length.saturating_add(count);
                }
            }
            Kind::Either(nodes)
        }
        Expr::Repeat(part, min, max) => {
            let node = Box::new(prepare_node(part, words));
            let mut counts = [0u128; MAX_LENGTH + 1];
            counts[0] = 1;
            for repeat in 0..=*max {
                if repeat >= *min {
                    for (length, count) in lengths.iter_mut().zip(counts) {
                        *length = length.saturating_add(count);
                    }
                }
                counts = product(&counts, &node.lengths);
            }
            Kind::Repeat(node, *min, *max)
        }
    };
    Node {
        kind,
        min: lengths
            .iter()
            .position(|count| *count != 0)
            .unwrap_or(MAX_LENGTH + 1),
        max: lengths.iter().rposition(|count| *count != 0).unwrap_or(0),
        lengths,
    }
}

impl Prepared {
    pub fn preview(&self, cancel: &AtomicBool) -> Result<Preview, String> {
        let mut samples = Vec::new();
        self.generate(cancel, |candidate| {
            samples.push(String::from_utf8_lossy(candidate).into_owned());
            Ok(samples.len() < 6)
        })
        .map_err(|e| e.to_string())?;
        Ok(Preview {
            total: self.total,
            words: self.word_count,
            samples,
        })
    }

    pub fn generate(
        &self,
        cancel: &AtomicBool,
        mut emit: impl FnMut(&[u8]) -> io::Result<bool>,
    ) -> io::Result<()> {
        walk(
            &mut vec![&self.root],
            &mut Vec::with_capacity(MAX_LENGTH),
            &self.words,
            cancel,
            &mut emit,
        )?;
        Ok(())
    }
}

fn walk<'a>(
    pending: &mut Vec<&'a Node>,
    candidate: &mut Vec<u8>,
    words: &Words,
    cancel: &AtomicBool,
    emit: &mut dyn FnMut(&[u8]) -> io::Result<bool>,
) -> io::Result<bool> {
    if cancel.load(Ordering::Relaxed) {
        return Ok(false);
    }
    let minimum = pending.iter().map(|node| node.min).sum::<usize>();
    let maximum = pending.iter().map(|node| node.max).sum::<usize>();
    if candidate.len() + minimum > MAX_LENGTH || candidate.len() + maximum < 8 {
        return Ok(true);
    }
    let Some(node) = pending.pop() else {
        return emit(candidate);
    };
    let stack_size = pending.len();
    let prefix_size = candidate.len();
    let mut more = true;
    match &node.kind {
        Kind::Literal(value) => {
            candidate.extend(value);
            more = walk(pending, candidate, words, cancel, emit)?;
            candidate.truncate(prefix_size);
        }
        Kind::Class(values) => {
            for value in values {
                candidate.push(*value);
                more = walk(pending, candidate, words, cancel, emit)?;
                candidate.truncate(prefix_size);
                if !more {
                    break;
                }
            }
        }
        Kind::Word(case) => {
            for value in &words[*case] {
                candidate.extend(value);
                more = walk(pending, candidate, words, cancel, emit)?;
                candidate.truncate(prefix_size);
                if !more {
                    break;
                }
            }
        }
        Kind::Concat(parts) => {
            pending.extend(parts.iter().rev());
            more = walk(pending, candidate, words, cancel, emit)?;
            pending.truncate(stack_size);
        }
        Kind::Either(parts) => {
            for part in parts {
                pending.push(part);
                more = walk(pending, candidate, words, cancel, emit)?;
                pending.truncate(stack_size);
                if !more {
                    break;
                }
            }
        }
        Kind::Repeat(part, min, max) => {
            for count in *min..=*max {
                pending.extend(std::iter::repeat_n(part.as_ref(), count as usize));
                more = walk(pending, candidate, words, cancel, emit)?;
                pending.truncate(stack_size);
                if !more {
                    break;
                }
            }
        }
    }
    pending.push(node);
    Ok(more)
}

pub fn format_count(value: u128) -> String {
    let digits = value.to_string();
    let mut result = String::new();
    if value == u128::MAX {
        result.push('≥');
    }
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(digit);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepare(source: &str, words: &[u8]) -> Prepared {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("words.txt");
        std::fs::write(&path, words).unwrap();
        Pattern::parse(source)
            .unwrap()
            .prepare(&path, &AtomicBool::new(false))
            .unwrap()
    }

    fn candidates(prepared: &Prepared) -> Vec<String> {
        let mut values = Vec::new();
        prepared
            .generate(&AtomicBool::new(false), |value| {
                values.push(String::from_utf8(value.to_vec()).unwrap());
                Ok(true)
            })
            .unwrap();
        assert_eq!(values.len() as u128, prepared.total);
        values
    }

    #[test]
    fn two_capitalized_words_and_three_digits_include_every_order_and_repeats() {
        let prepared = prepare("{Word}{Word}[0-9]{3}", b"aLPha\r\nbeta\n");
        assert_eq!(prepared.total, 4_000);
        let values = candidates(&prepared);
        assert_eq!(values.first().unwrap(), "AlphaAlpha000");
        assert_eq!(values.last().unwrap(), "BetaBeta999");
        assert!(values.contains(&"AlphaBeta123".into()));
        assert!(values.contains(&"BetaAlpha123".into()));
    }

    #[test]
    fn five_word_repeats_and_separators_are_independent_choices() {
        let prepared = prepare("{word}{5}", b"ab\ncd\n");
        let values = candidates(&prepared);
        assert_eq!(prepared.total, 32);
        assert_eq!(values.first().unwrap(), "ababababab");
        assert_eq!(values.last().unwrap(), "cdcdcdcdcd");
        let separated = candidates(&prepare("({word}-){4}{word}", b"ab\ncd\n"));
        assert_eq!(separated.first().unwrap(), "ab-ab-ab-ab-ab");
        assert_eq!(separated.len(), 32);
    }

    #[test]
    fn alternatives_optional_suffixes_anchors_and_escaped_literals_work() {
        let prepared = prepare("^(Sun|Moon)-{lower}[0-1]{2}!?$", b"CAT\nDOG\n");
        let values = candidates(&prepared);
        assert_eq!(prepared.total, 32);
        assert!(values.contains(&"Sun-cat00".into()));
        assert!(values.contains(&"Moon-dog11!".into()));
        let literal = Pattern::parse(r"\{word\}xx").unwrap();
        assert!(!literal.needs_words());
        let prepared = literal
            .prepare(Path::new("/missing-wordlist"), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(candidates(&prepared), ["{word}xx"]);
    }

    #[test]
    fn counts_and_generation_filter_by_complete_password_byte_length() {
        let values = candidates(&prepare("{word}{2}", b"a\nsevenxx\n"));
        assert_eq!(values, ["asevenxx", "sevenxxa", "sevenxxsevenxx"]);
        let words = format!("\ncafé\n{}\n", "a".repeat(64));
        let prepared = prepare("{word}{2}", words.as_bytes());
        assert_eq!(prepared.word_count, 1);
        assert_eq!(candidates(&prepared), ["cafécafé"]);
        let too_short = Pattern::parse("[0-9]{7}").unwrap();
        assert!(
            too_short
                .prepare(Path::new(""), &AtomicBool::new(false))
                .is_err()
        );
    }

    #[test]
    fn huge_searches_can_be_counted_sampled_and_cancelled_without_expansion() {
        let prepared = prepare("[a-z]{63}", b"");
        assert_eq!(prepared.total, u128::MAX);
        let preview = prepared.preview(&AtomicBool::new(false)).unwrap();
        assert_eq!(preview.samples.len(), 6);
        assert_eq!(preview.samples[0], "a".repeat(63));
        let cancel = AtomicBool::new(false);
        let mut emitted = 0;
        prepared
            .generate(&cancel, |_| {
                emitted += 1;
                cancel.store(true, Ordering::Relaxed);
                Ok(true)
            })
            .unwrap();
        assert_eq!(emitted, 1);
    }

    #[test]
    fn unsupported_or_unbounded_syntax_is_rejected() {
        for source in [
            "",
            "a*",
            "a+",
            "a{1,}",
            "a{64}",
            "[",
            "(?=a)",
            r"(a)\1",
            "(?i:{word})",
            "((a?){63}){63}",
            r"literal\n",
        ] {
            assert!(Pattern::parse(source).is_err(), "accepted {source}");
        }
    }
}
