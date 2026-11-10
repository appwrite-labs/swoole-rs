//! The ECMAScript regex dialect `url_rewrite_rules` patterns are written in, over bytes:
//! what `std::regex` accepts, less backreferences and lookahead (a pattern with either
//! doesn't compile, so its rule never matches). Matching is leftmost-first, the order a
//! backtracking engine tries alternatives in, on a Pike VM: time linear in the URL, and no
//! recursion while matching.

use std::ffi::c_char;

/// libstdc++'s `_GLIBCXX_REGEX_STATE_LIMIT`: a larger program is `error_space`.
const LIMIT: usize = 100_000;
/// How deeply groups nest, so parsing and compiling recurse a bounded depth.
const NESTING: usize = 250;

/// A set of bytes: bytes 0-127, then 128-255.
#[derive(Clone, Copy, Default)]
struct Set(u128, u128);

impl Set {
    fn range(self, low: u8, high: u8) -> Self {
        (low..=high).fold(self, |set, byte| match byte.checked_sub(128) {
            None => Self(set.0 | 1 << byte, set.1),
            Some(high) => Self(set.0, set.1 | 1 << high),
        })
    }
    fn byte(self, byte: u8) -> Self {
        self.range(byte, byte)
    }
    fn union(self, other: Self) -> Self {
        Self(self.0 | other.0, self.1 | other.1)
    }
    fn minus(self, other: Self) -> Self {
        Self(self.0 & !other.0, self.1 & !other.1)
    }
    fn not(self) -> Self {
        Self(!self.0, !self.1)
    }
    fn has(self, byte: u8) -> bool {
        match byte.checked_sub(128) {
            None => self.0 >> byte & 1 != 0,
            Some(high) => self.1 >> high & 1 != 0,
        }
    }
    fn digit() -> Self {
        Self::default().range(b'0', b'9')
    }
    fn word() -> Self {
        Self::digit().range(b'A', b'Z').range(b'a', b'z').byte(b'_')
    }
    /// `\s`: space, `\t`, `\n`, `\v`, `\f` and `\r`.
    fn space() -> Self {
        Self::default().byte(b' ').range(b'\t', b'\r')
    }
    /// `[:name:]`: libstdc++'s `regex_traits::lookup_classname` in the C locale, which
    /// lowercases the name first.
    fn named(name: &[u8]) -> Option<Self> {
        let alpha = Self::default().range(b'A', b'Z').range(b'a', b'z');
        let alnum = alpha.union(Self::digit());
        let graph = Self::default().range(b'!', b'~');
        Some(match name.to_ascii_lowercase().as_slice() {
            b"alnum" => alnum,
            b"alpha" => alpha,
            b"blank" => Self::default().byte(b' ').byte(b'\t'),
            b"cntrl" => Self::default().range(0, 0x1f).byte(0x7f),
            b"d" | b"digit" => Self::digit(),
            b"graph" => graph,
            b"lower" => Self::default().range(b'a', b'z'),
            b"print" => graph.byte(b' '),
            b"punct" => graph.minus(alnum),
            b"s" | b"space" => Self::space(),
            b"upper" => Self::default().range(b'A', b'Z'),
            b"w" => Self::word(),
            b"xdigit" => Self::digit().range(b'A', b'F').range(b'a', b'f'),
            _ => return None,
        })
    }
}

#[derive(Clone, Copy)]
enum Look {
    Start,
    End,
    Boundary,
    NotBoundary,
}

impl Look {
    /// Whether the assertion holds at `at`, where `hay` starts at `begin`.
    fn holds(self, hay: &[u8], begin: usize, at: usize) -> bool {
        let word = |i: usize| i >= begin && hay.get(i).is_some_and(|&byte| Set::word().has(byte));
        let boundary = at.checked_sub(1).is_some_and(word) != word(at);
        match self {
            Self::Start => at == begin,
            Self::End => at == hay.len(),
            Self::Boundary => boundary,
            Self::NotBoundary => !boundary,
        }
    }
}

enum Node {
    Set(Set),
    Look(Look),
    Group(usize, Box<Node>),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    /// The node, its minimum and maximum count, and whether it is greedy.
    Repeat(Box<Node>, u32, Option<u32>, bool),
}

impl Node {
    /// Whether the node can match the empty string.
    fn empty(&self) -> bool {
        match self {
            Self::Set(_) => false,
            Self::Look(_) => true,
            Self::Group(_, node) => node.empty(),
            Self::Concat(nodes) => nodes.iter().all(Self::empty),
            Self::Alt(nodes) => nodes.iter().any(Self::empty),
            Self::Repeat(node, min, ..) => *min == 0 || node.empty(),
        }
    }
}

/// Each method returns `None` where `std::regex` throws.
struct Parser<'a> {
    rest: &'a [u8],
    groups: usize,
    depth: usize,
}

impl Parser<'_> {
    fn eat(&mut self, byte: u8) -> bool {
        let found = self.rest.first() == Some(&byte);
        if found {
            self.skip();
        }
        found
    }
    fn skip(&mut self) {
        self.rest = self.rest.get(1..).unwrap_or_default();
    }
    fn next(&mut self) -> Option<u8> {
        let (&byte, rest) = self.rest.split_first()?;
        self.rest = rest;
        Some(byte)
    }
    fn alternation(&mut self) -> Option<Node> {
        let mut alternatives = vec![self.concat()?];
        while self.eat(b'|') {
            alternatives.push(self.concat()?);
        }
        Some(if alternatives.len() == 1 {
            alternatives.remove(0)
        } else {
            Node::Alt(alternatives)
        })
    }
    fn concat(&mut self) -> Option<Node> {
        let mut terms = Vec::new();
        while !matches!(self.rest.first(), None | Some(b'|' | b')')) {
            terms.push(self.term()?);
        }
        Some(Node::Concat(terms))
    }
    fn term(&mut self) -> Option<Node> {
        let atom = match self.next()? {
            b'^' => return Some(Node::Look(Look::Start)),
            b'$' => return Some(Node::Look(Look::End)),
            b'\\' if self.eat(b'b') => return Some(Node::Look(Look::Boundary)),
            b'\\' if self.eat(b'B') => return Some(Node::Look(Look::NotBoundary)),
            b'\\' => Node::Set(self.escape()?),
            b'.' => Node::Set(Set::default().byte(b'\n').byte(b'\r').not()),
            b'[' => Node::Set(self.class()?),
            b'(' => self.group()?,
            b'*' | b'+' | b'?' | b'{' => return None,
            byte => Node::Set(Set::default().byte(byte)),
        };
        // libstdc++ takes any number of quantifiers after an atom: `a**` is `(?:a*)*`.
        // Each nests a level, which counts toward NESTING.
        let (mut node, depth) = (atom, self.depth);
        loop {
            let (min, max) = match self.rest.first() {
                Some(b'*') => (0, None),
                Some(b'+') => (1, None),
                Some(b'?') => (0, Some(1)),
                Some(b'{') => {
                    self.skip();
                    let min = self.number()?;
                    let max = match (self.eat(b','), self.rest.first()) {
                        (false, _) => Some(min),
                        (true, Some(b'}')) => None,
                        (true, _) => Some(self.number()?),
                    };
                    if self.rest.first() != Some(&b'}') || max.is_some_and(|max| max < min) {
                        return None;
                    }
                    (min, max)
                }
                _ => {
                    self.depth = depth;
                    return Some(node);
                }
            };
            self.depth += 1;
            if self.depth > NESTING {
                return None;
            }
            // The quantifier's last byte, then `?` for a lazy one.
            self.skip();
            node = Node::Repeat(Box::new(node), min, max, !self.eat(b'?'));
        }
    }
    fn group(&mut self) -> Option<Node> {
        self.depth += 1;
        if self.depth > NESTING {
            return None;
        }
        let node = if self.eat(b'?') {
            // `(?:`; lookahead is not supported.
            if !self.eat(b':') {
                return None;
            }
            self.alternation()?
        } else {
            self.groups += 1;
            let index = self.groups;
            Node::Group(index, Box::new(self.alternation()?))
        };
        self.depth -= 1;
        self.eat(b')').then_some(node)
    }
    fn number(&mut self) -> Option<u32> {
        let digits = self.rest.iter().take_while(|b| b.is_ascii_digit()).count();
        let (number, rest) = self.rest.split_at(digits);
        self.rest = rest;
        std::str::from_utf8(number).ok()?.parse().ok()
    }
    /// The escape after a `\`, as libstdc++'s `_M_eat_escape_ecma` reads it: a class
    /// escape, a control, or else the byte itself. `\1`-`\9` would be backreferences,
    /// which are not supported. Outside a class `term` has read `\b` and `\B` already;
    /// inside one, `\b` is a backspace and `\B` an error.
    fn escape(&mut self) -> Option<Set> {
        let set = |byte| Some(Set::default().byte(byte));
        match self.next()? {
            b'd' => Some(Set::digit()),
            b'D' => Some(Set::digit().not()),
            b'w' => Some(Set::word()),
            b'W' => Some(Set::word().not()),
            b's' => Some(Set::space()),
            b'S' => Some(Set::space().not()),
            b'0' => set(0),
            b'b' => set(8),
            b't' => set(b'\t'),
            b'n' => set(b'\n'),
            b'v' => set(0x0b),
            b'f' => set(0x0c),
            b'r' => set(b'\r'),
            // libstdc++ reads `\cX` as `X`.
            b'c' => set(self.next()?),
            // `\xHH`, and the low byte of `\uHHHH`, which is all libstdc++ keeps of it.
            escape @ (b'x' | b'u') => {
                let digits = if escape == b'x' { 2 } else { 4 };
                let hex = self.rest.get(..digits)?;
                self.rest = self.rest.get(digits..)?;
                if !hex.iter().all(u8::is_ascii_hexdigit) {
                    return None;
                }
                let low = std::str::from_utf8(hex.get(digits - 2..)?).ok()?;
                set(u8::from_str_radix(low, 16).ok()?)
            }
            b'1'..=b'9' | b'B' => None,
            byte => set(byte),
        }
    }
    /// After `[`: `[]` matches nothing and `[^]` anything, as in ECMAScript.
    fn class(&mut self) -> Option<Set> {
        let negated = self.eat(b'^');
        let mut set = Set::default();
        while !self.eat(b']') {
            let (member, low) = self.class_atom()?;
            if self.rest.first() == Some(&b'-') && self.rest.get(1).is_some_and(|&b| b != b']') {
                self.skip();
                // A range's ends are single bytes: `[\d-z]` is an error. libstdc++ compares
                // the ends, and each byte with them, as `char`, which is signed on x86_64:
                // there `[a-\xff]` is an error and `[\xff-a]` holds 0xff and 0x00-0x61.
                let char = |byte: u8| c_char::from_ne_bytes([byte]);
                let (low, high) = (char(low?), char(self.class_atom()?.1?));
                if low > high {
                    return None;
                }
                set = (0..=255)
                    .filter(|&byte| (low..=high).contains(&char(byte)))
                    .fold(set, Set::byte);
            } else {
                set = set.union(member);
            }
        }
        Some(if negated { set.not() } else { set })
    }
    /// A class member, and its byte when it is one byte.
    fn class_atom(&mut self) -> Option<(Set, Option<u8>)> {
        if let Some(rest) = self.rest.strip_prefix(b"[:") {
            let end = rest.windows(2).position(|pair| pair == b":]")?;
            let name = rest.get(..end)?;
            self.rest = rest.get(end + 2..)?;
            return Some((Set::named(name)?, None));
        }
        // Collating elements and equivalence classes are not supported.
        if self.rest.starts_with(b"[.") || self.rest.starts_with(b"[=") {
            return None;
        }
        let byte = self.next()?;
        if byte != b'\\' {
            return Some((Set::default().byte(byte), Some(byte)));
        }
        let set = self.escape()?;
        let mut members = (0..=255u8).filter(|&b| set.has(b));
        let byte = match (members.next(), members.next()) {
            (Some(byte), None) => Some(byte),
            _ => None,
        };
        Some((set, byte))
    }
}

#[derive(Clone, Copy)]
enum Inst {
    Set(Set),
    Look(Look),
    /// Try the first branch, then the second.
    Split(usize, usize),
    Jmp(usize),
    Save(usize),
    Match,
}

/// A compiled pattern.
pub(super) struct Regex {
    program: Vec<Inst>,
    slots: usize,
}

/// A match's group spans: start and end of each group, group 0 the whole match.
pub(super) struct Captures(Vec<Option<usize>>);

impl Captures {
    /// The span group `index` matched, if it took part in the match.
    pub(super) fn group(&self, index: usize) -> Option<std::ops::Range<usize>> {
        let start = (*self.0.get(2 * index)?)?;
        Some(start..(*self.0.get(2 * index + 1)?)?)
    }
}

type Slots = Vec<Option<usize>>;

impl Regex {
    /// `None` where `std::regex` throws, and for a backreference or lookahead.
    pub(super) fn new(pattern: &[u8]) -> Option<Self> {
        let mut parser = Parser {
            rest: pattern,
            groups: 0,
            depth: 0,
        };
        let node = parser.alternation()?;
        if !parser.rest.is_empty() {
            return None; // an unmatched `)`
        }
        let mut regex = Self {
            program: vec![Inst::Save(0)],
            slots: 2 * parser.groups + 2,
        };
        regex.emit(&node)?;
        regex.program.extend([Inst::Save(1), Inst::Match]);
        Some(regex)
    }

    fn patch(&mut self, at: usize, inst: Inst) {
        if let Some(slot) = self.program.get_mut(at) {
            *slot = inst;
        }
    }

    fn push(&mut self, inst: Inst) -> Option<usize> {
        self.program.push(inst);
        (self.program.len() <= LIMIT).then_some(self.program.len() - 1)
    }

    fn emit(&mut self, node: &Node) -> Option<()> {
        let before = self.program.len();
        match node {
            Node::Set(set) => {
                self.push(Inst::Set(*set))?;
            }
            Node::Look(look) => {
                self.push(Inst::Look(*look))?;
            }
            Node::Group(index, node) => {
                self.push(Inst::Save(2 * index))?;
                self.emit(node)?;
                self.push(Inst::Save(2 * index + 1))?;
            }
            Node::Concat(nodes) => nodes.iter().try_for_each(|node| self.emit(node))?,
            Node::Alt(nodes) => {
                let (last, init) = nodes.split_last()?;
                let mut jumps = Vec::new();
                for node in init {
                    let split = self.push(Inst::Split(0, 0))?;
                    self.emit(node)?;
                    jumps.push(self.push(Inst::Jmp(0))?);
                    self.patch(split, Inst::Split(split + 1, self.program.len()));
                }
                self.emit(last)?;
                let end = self.program.len();
                for jump in jumps {
                    self.patch(jump, Inst::Jmp(end));
                }
            }
            Node::Repeat(node, min, max, greedy) => {
                let order = |body, out| {
                    if *greedy {
                        Inst::Split(body, out)
                    } else {
                        Inst::Split(out, body)
                    }
                };
                // Each optional copy skips to the end: `x{1,3}` is `x(?:x(?:x)?)?`, and
                // `x{2,}` is `xx+`. `x*` loops on one split, or is `(?:x+)?` when `x` can
                // match empty, so an iteration that matches empty still sets its groups.
                if *min == 0 && max.is_none() && !node.empty() {
                    let split = self.push(Inst::Split(0, 0))?;
                    self.emit(node)?;
                    self.push(Inst::Jmp(split))?;
                    self.patch(split, order(split + 1, self.program.len()));
                    return Some(());
                }
                let (copies, optional) = match max {
                    Some(max) => (*min, max - min),
                    None => (min.saturating_sub(1), u32::from(*min == 0)),
                };
                for _ in 0..copies {
                    self.emit(node)?;
                }
                let mut splits = Vec::new();
                for _ in 0..optional {
                    splits.push(self.push(Inst::Split(0, 0))?);
                    self.emit(node)?;
                }
                if max.is_none() {
                    let body = splits.first().map_or(self.program.len(), |split| split + 1);
                    if splits.is_empty() {
                        self.emit(node)?;
                    }
                    let repeat = self.push(Inst::Split(0, 0))?;
                    self.patch(repeat, order(body, repeat + 1));
                }
                let end = self.program.len();
                for split in splits {
                    self.patch(split, order(split + 1, end));
                }
            }
        }
        // Every node adds a state, as each of libstdc++'s does, so one with no instructions,
        // such as `(?:)` or `a{0}`, still counts toward the limit: `(?:){100000}` is over it.
        if self.program.len() == before {
            self.push(Inst::Jmp(before + 1))?;
        }
        Some(())
    }

    /// The leftmost-first match in `hay` at or after `start`. An `anchored` search is
    /// libstdc++'s `match_continuous | match_not_null`: the first non-empty match at
    /// `start`, in priority order. `^` and `\b` see `hay` from `begin`: 0, or `start`
    /// without `match_prev_avail`.
    pub(super) fn search(
        &self,
        hay: &[u8],
        begin: usize,
        start: usize,
        anchored: bool,
    ) -> Option<Captures> {
        // The position each instruction was last added at, so each is added once per byte.
        let mut seen = vec![usize::MAX; self.program.len()];
        let (mut current, mut next) = (Vec::new(), Vec::new());
        let mut found = None;
        for at in start..=hay.len() {
            if found.is_none() && (!anchored || at == start) {
                self.follow(
                    &mut current,
                    &mut seen,
                    0,
                    vec![None; self.slots],
                    (hay, begin),
                    at,
                );
            }
            if current.is_empty() && (found.is_some() || anchored) {
                break;
            }
            for (pc, slots) in current.drain(..) {
                match self.program.get(pc) {
                    Some(Inst::Set(set)) if hay.get(at).is_some_and(|&byte| set.has(byte)) => {
                        self.follow(&mut next, &mut seen, pc + 1, slots, (hay, begin), at + 1);
                    }
                    // Threads after this one have lower priority.
                    Some(Inst::Match) if !anchored || at > start => {
                        found = Some(Captures(slots));
                        break;
                    }
                    _ => {}
                }
            }
            std::mem::swap(&mut current, &mut next);
        }
        found
    }

    /// Adds the threads `pc` reaches at `at` without consuming a byte, in priority order.
    fn follow(
        &self,
        list: &mut Vec<(usize, Slots)>,
        seen: &mut [usize],
        pc: usize,
        slots: Slots,
        (hay, begin): (&[u8], usize),
        at: usize,
    ) {
        let mut stack = vec![(pc, slots)];
        while let Some((pc, mut slots)) = stack.pop() {
            let (Some(last), Some(&inst)) = (seen.get_mut(pc), self.program.get(pc)) else {
                continue;
            };
            if std::mem::replace(last, at) == at {
                continue;
            }
            match inst {
                Inst::Jmp(to) => stack.push((to, slots)),
                Inst::Split(first, second) => {
                    stack.push((second, slots.clone()));
                    stack.push((first, slots));
                }
                Inst::Save(slot) => {
                    if let Some(slot) = slots.get_mut(slot) {
                        *slot = Some(at);
                    }
                    stack.push((pc + 1, slots));
                }
                Inst::Look(look) => {
                    if look.holds(hay, begin, at) {
                        stack.push((pc + 1, slots));
                    }
                }
                Inst::Set(_) | Inst::Match => list.push((pc, slots)),
            }
        }
    }
}
