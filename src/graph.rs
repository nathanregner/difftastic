//! A graph representation for computing tree diffs.

use rustc_hash::FxHasher;
use std::{
    fmt,
    hash::{Hash, Hasher},
};
use strsim::normalized_levenshtein;

use crate::syntax::{AtomKind, ChangeKind, Syntax};
use Edge::*;

/// A vertex in a directed acyclic graph that represents a diff.
///
/// Each vertex represents two pointers: one to the next unmatched LHS
/// syntax, and one to the next unmatched RHS syntax.
///
/// For example, suppose we have `X A` on the LHS and `A` on the
/// RHS. Our start vertex looks like this.
///
/// ```text
/// LHS: X A     RHS: A
///      ^            ^
/// ```
///
/// From this vertex, we could take [`Edge::NovelAtomLHS`], bringing
/// us to this vertex.
///
/// ```text
/// LHS: X A     RHS: A
///        ^          ^
/// ```
///
/// Alternatively, we could take the [`Edge::NovelAtomRHS`], bringing us
/// to this vertex.
///
/// ```text
/// LHS: X A     RHS: A
///      ^              ^
/// ```
#[derive(Debug, Clone)]
pub struct Vertex<'a> {
    pub lhs_syntax: Option<&'a Syntax<'a>>,
    pub rhs_syntax: Option<&'a Syntax<'a>>,
    parents: rpds::Stack<EnteredDelimiter<'a>>,
    parents_hash: u64,
}

impl<'a> PartialEq for Vertex<'a> {
    fn eq(&self, other: &Self) -> bool {
        self.lhs_syntax.map(|node| node.id()) == other.lhs_syntax.map(|node| node.id())
            && self.rhs_syntax.map(|node| node.id()) == other.rhs_syntax.map(|node| node.id())
            && self.parents_hash == other.parents_hash
            && self.parents == other.parents
    }
}
impl<'a> Eq for Vertex<'a> {}

impl<'a> Hash for Vertex<'a> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.lhs_syntax.map(|node| node.id()).hash(state);
        self.rhs_syntax.map(|node| node.id()).hash(state);

        self.parents_hash.hash(state);
    }
}

// Compare LHS and RHS parents separately. This ensures that
// the following are considered equal:
//
// [EnterNovelDelimiterLHS, EnterNovelDelimiterRHS]
// [EnterNovelDelimiterRHS, EnterNovelDelimiterLHS]
//
// Otherwise we would construct a much bigger graph and
// difftastic wouldn't scale to medium size programs such as
// sample_files/nest_after.rs.
#[derive(Clone)]
enum EnteredDelimiter<'a> {
    PopEither((rpds::Stack<&'a Syntax<'a>>, rpds::Stack<&'a Syntax<'a>>)),
    PopBoth((&'a Syntax<'a>, &'a Syntax<'a>)),
}

impl<'a> fmt::Debug for EnteredDelimiter<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let desc = match self {
            EnteredDelimiter::PopEither((lhs_delims, rhs_delims)) => {
                format!("PopEither({}, {})", lhs_delims.size(), rhs_delims.size())
            }
            EnteredDelimiter::PopBoth(_) => "PopBoth".to_string(),
        };
        f.write_str(&desc)
    }
}

impl<'a> PartialEq for EnteredDelimiter<'a> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                EnteredDelimiter::PopEither((self_lhs_delims, self_rhs_delims)),
                EnteredDelimiter::PopEither((other_lhs_delims, other_rhs_delims)),
            ) => {
                if self_lhs_delims.size() != other_lhs_delims.size() {
                    return false;
                }
                if self_rhs_delims.size() != other_rhs_delims.size() {
                    return false;
                }
                for (self_lhs_delim, other_lhs_delim) in
                    self_lhs_delims.iter().zip(other_lhs_delims.iter())
                {
                    if self_lhs_delim.id() != other_lhs_delim.id() {
                        return false;
                    }
                }
                for (self_rhs_delim, other_rhs_delim) in
                    self_rhs_delims.iter().zip(other_rhs_delims.iter())
                {
                    if self_rhs_delim.id() != other_rhs_delim.id() {
                        return false;
                    }
                }
                true
            }
            (
                EnteredDelimiter::PopBoth((self_lhs_delim, self_rhs_delim)),
                EnteredDelimiter::PopBoth((other_lhs_delim, other_rhs_delim)),
            ) => {
                self_lhs_delim.id() == other_lhs_delim.id()
                    && self_rhs_delim.id() == other_rhs_delim.id()
            }
            _ => false,
        }
    }
}
impl<'a> Eq for EnteredDelimiter<'a> {}

fn push_both_delimiters<'a>(
    entered: &rpds::Stack<EnteredDelimiter<'a>>,
    lhs_delim: &'a Syntax<'a>,
    rhs_delim: &'a Syntax<'a>,
) -> rpds::Stack<EnteredDelimiter<'a>> {
    entered.push(EnteredDelimiter::PopBoth((lhs_delim, rhs_delim)))
}

fn try_pop_both<'a>(
    entered: &rpds::Stack<EnteredDelimiter<'a>>,
) -> Option<(
    &'a Syntax<'a>,
    &'a Syntax<'a>,
    rpds::Stack<EnteredDelimiter<'a>>,
)> {
    match entered.peek() {
        Some(EnteredDelimiter::PopBoth((lhs_delim, rhs_delim))) => {
            Some((lhs_delim, rhs_delim, entered.pop().unwrap()))
        }
        _ => None,
    }
}

fn try_pop_lhs<'a>(
    entered: &rpds::Stack<EnteredDelimiter<'a>>,
) -> Option<(&'a Syntax<'a>, rpds::Stack<EnteredDelimiter<'a>>)> {
    match entered.peek() {
        Some(EnteredDelimiter::PopEither((lhs_delims, rhs_delims))) => match lhs_delims.peek() {
            Some(lhs_delim) => {
                let mut entered = entered.clone();
                entered = entered.pop().unwrap();

                let new_lhs_delims = lhs_delims.pop().unwrap();

                if !new_lhs_delims.is_empty() || !rhs_delims.is_empty() {
                    entered = entered.push(EnteredDelimiter::PopEither((
                        new_lhs_delims,
                        rhs_delims.clone(),
                    )));
                }

                Some((lhs_delim, entered))
            }
            None => None,
        },
        _ => None,
    }
}

fn try_pop_rhs<'a>(
    entered: &rpds::Stack<EnteredDelimiter<'a>>,
) -> Option<(&'a Syntax<'a>, rpds::Stack<EnteredDelimiter<'a>>)> {
    match entered.peek() {
        Some(EnteredDelimiter::PopEither((lhs_delims, rhs_delims))) => match rhs_delims.peek() {
            Some(rhs_delim) => {
                let mut entered = entered.clone();
                entered = entered.pop().unwrap();

                let new_rhs_delims = rhs_delims.pop().unwrap();

                if !lhs_delims.is_empty() || !new_rhs_delims.is_empty() {
                    entered = entered.push(EnteredDelimiter::PopEither((
                        lhs_delims.clone(),
                        new_rhs_delims,
                    )));
                }

                Some((rhs_delim, entered))
            }
            None => None,
        },
        _ => None,
    }
}

fn push_lhs_delimiter<'a>(
    entered: &rpds::Stack<EnteredDelimiter<'a>>,
    delimiter: &'a Syntax<'a>,
) -> rpds::Stack<EnteredDelimiter<'a>> {
    let mut entered = entered.clone();

    let mut modifying_head = false;
    let (mut lhs_delims, rhs_delims) = match entered.peek() {
        Some(EnteredDelimiter::PopEither((lhs_delims, rhs_delims))) => {
            modifying_head = true;
            (lhs_delims.clone(), rhs_delims.clone())
        }
        _ => (rpds::Stack::new(), rpds::Stack::new()),
    };
    lhs_delims = lhs_delims.push(delimiter);

    if modifying_head {
        entered = entered.pop().unwrap();
    }
    entered.push(EnteredDelimiter::PopEither((lhs_delims, rhs_delims)))
}

fn push_rhs_delimiter<'a>(
    entered: &rpds::Stack<EnteredDelimiter<'a>>,
    delimiter: &'a Syntax<'a>,
) -> rpds::Stack<EnteredDelimiter<'a>> {
    let mut entered = entered.clone();

    let mut modifying_head = false;
    let (lhs_delims, mut rhs_delims) = match entered.peek() {
        Some(EnteredDelimiter::PopEither((lhs_delims, rhs_delims))) => {
            modifying_head = true;
            (lhs_delims.clone(), rhs_delims.clone())
        }
        _ => (rpds::Stack::new(), rpds::Stack::new()),
    };
    rhs_delims = rhs_delims.push(delimiter);

    if modifying_head {
        entered = entered.pop().unwrap();
    }
    entered.push(EnteredDelimiter::PopEither((lhs_delims, rhs_delims)))
}

fn hash_parents(parents: &rpds::Stack<EnteredDelimiter>) -> u64 {
    let mut hasher = FxHasher::default();

    for entered in parents.iter() {
        match entered {
            EnteredDelimiter::PopEither((lhs_delims, rhs_delims)) => {
                // FxHasher finishes with 0 if called with
                // .write_u32(0). Ensure the u32 written is always
                // non-zero.
                for lhs_delim in lhs_delims {
                    hasher.write_u32(lhs_delim.id() + 1);
                }
                for rhs_delim in rhs_delims {
                    hasher.write_u32(rhs_delim.id() + 1);
                }
            }
            EnteredDelimiter::PopBoth((lhs_delim, rhs_delim)) => {
                hasher.write_u32(lhs_delim.id() + 1);
                hasher.write_u32(rhs_delim.id() + 1);
            }
        }
    }

    hasher.finish()
}

impl<'a> Vertex<'a> {
    pub fn is_end(&self) -> bool {
        self.lhs_syntax.is_none() && self.rhs_syntax.is_none() && self.parents.is_empty()
    }

    pub fn new(lhs_syntax: Option<&'a Syntax<'a>>, rhs_syntax: Option<&'a Syntax<'a>>) -> Self {
        let parents = rpds::Stack::new();
        let parents_hash = hash_parents(&parents);
        Vertex {
            lhs_syntax,
            rhs_syntax,
            parents,
            parents_hash,
        }
    }
}

/// An edge in our graph, with an associated [`cost`](Edge::cost).
///
/// A syntax node can always be marked as novel, so a vertex will have
/// at least a NovelFoo edge. Depending on the syntax nodes of the
/// current [`Vertex`], other edges may also be available.
///
/// See [`neighbours`] for all the edges available for a given `Vertex`.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Edge {
    UnchangedNode,
    EnterUnchangedDelimiter,
    ReplacedComment { levenshtein_pct: u8 },
    NovelAtomLHS { contiguous: bool },
    NovelAtomRHS { contiguous: bool },
    // TODO: An EnterNovelDelimiterBoth edge might help performance
    // rather doing LHS and RHS separately.
    EnterNovelDelimiterLHS { contiguous: bool },
    EnterNovelDelimiterRHS { contiguous: bool },
    NovelTreeLHS { num_descendants: u32 },
    NovelTreeRHS { num_descendants: u32 },
    ExitDelimiterLHS,
    ExitDelimiterRHS,
    ExitDelimiterBoth,
}

impl Edge {
    pub fn cost(&self) -> u64 {
        match self {
            ExitDelimiterBoth => 1,
            ExitDelimiterLHS | ExitDelimiterRHS => 2,

            // Matching nodes is always best.
            UnchangedNode => 1,
            // Matching an outer delimiter is good.
            EnterUnchangedDelimiter => 100,

            // Replacing a comment is better than treating it as novel.
            ReplacedComment { levenshtein_pct } => 150 + u64::from(100 - levenshtein_pct),

            // Otherwise, we've added/removed a node.
            NovelAtomLHS { contiguous }
            | NovelAtomRHS { contiguous }
            | EnterNovelDelimiterLHS { contiguous }
            | EnterNovelDelimiterRHS { contiguous } => {
                if *contiguous {
                    300
                } else {
                    // This needs to be more than 40 greater than the
                    // contiguous case. Otherwise, we end up choosing
                    // a badly positioned unchanged delimiter just
                    // because it has a better depth difference.
                    //
                    // TODO: write a test for this case.
                    350
                }
            }

            // For large trees, it's better to mark the whole tree as
            // novel rather than marking 90% of the children as
            // novel. This stops us matching up completely unrelated trees.
            NovelTreeLHS { num_descendants } | NovelTreeRHS { num_descendants } => {
                300 + (*num_descendants as u64 - 10) * NovelAtomLHS { contiguous: false }.cost()
            }
        }
    }
}

const NOVEL_TREE_THRESHOLD: u32 = 20;

/// Calculate all the neighbours from `v` and write them to `buf`.
pub fn neighbours<'a>(v: &Vertex<'a>, buf: &mut [Option<(Edge, Vertex<'a>)>]) {
    for item in &mut *buf {
        *item = None;
    }

    let mut i = 0;

    if v.lhs_syntax.is_none() && v.rhs_syntax.is_none() {
        if let Some((lhs_parent, rhs_parent, parents_next)) = try_pop_both(&v.parents) {
            // We have exhausted all the nodes on both lists, so we can
            // move up to the parent node.
            let parents_hash = hash_parents(&parents_next);

            // Continue from sibling of parent.
            buf[i] = Some((
                ExitDelimiterBoth,
                Vertex {
                    lhs_syntax: lhs_parent.next_sibling(),
                    rhs_syntax: rhs_parent.next_sibling(),
                    parents: parents_next,
                    parents_hash,
                },
            ));
            i += 1;
        }
    }

    if v.lhs_syntax.is_none() {
        if let Some((lhs_parent, parents_next)) = try_pop_lhs(&v.parents) {
            // Move to next after LHS parent.
            let parents_hash = hash_parents(&parents_next);

            // Continue from sibling of parent.
            buf[i] = Some((
                ExitDelimiterLHS,
                Vertex {
                    lhs_syntax: lhs_parent.next_sibling(),
                    rhs_syntax: v.rhs_syntax,
                    parents: parents_next,
                    parents_hash,
                },
            ));
            i += 1;
        }
    }

    if v.rhs_syntax.is_none() {
        if let Some((rhs_parent, parents_next)) = try_pop_rhs(&v.parents) {
            // Move to next after RHS parent.
            let parents_hash = hash_parents(&parents_next);

            // Continue from sibling of parent.
            buf[i] = Some((
                ExitDelimiterRHS,
                Vertex {
                    lhs_syntax: v.lhs_syntax,
                    rhs_syntax: rhs_parent.next_sibling(),
                    parents: parents_next,
                    parents_hash,
                },
            ));
            i += 1;
        }
    }

    if let (Some(lhs_syntax), Some(rhs_syntax)) = (&v.lhs_syntax, &v.rhs_syntax) {
        if lhs_syntax == rhs_syntax {
            // Both nodes are equal, the happy case.
            buf[i] = Some((
                UnchangedNode,
                Vertex {
                    lhs_syntax: lhs_syntax.next_sibling(),
                    rhs_syntax: rhs_syntax.next_sibling(),
                    parents: v.parents.clone(),
                    parents_hash: v.parents_hash,
                },
            ));
            return;
        }

        if let (
            Syntax::List {
                open_content: lhs_open_content,
                close_content: lhs_close_content,
                children: lhs_children,
                ..
            },
            Syntax::List {
                open_content: rhs_open_content,
                close_content: rhs_close_content,
                children: rhs_children,
                ..
            },
        ) = (lhs_syntax, rhs_syntax)
        {
            // The list delimiters are equal, but children may not be.
            if lhs_open_content == rhs_open_content && lhs_close_content == rhs_close_content {
                let lhs_next = lhs_children.get(0).copied();
                let rhs_next = rhs_children.get(0).copied();

                // TODO: be consistent between parents_next and next_parents.
                let parents_next = push_both_delimiters(&v.parents, lhs_syntax, rhs_syntax);
                let parents_hash = hash_parents(&parents_next);

                buf[i] = Some((
                    EnterUnchangedDelimiter,
                    Vertex {
                        lhs_syntax: lhs_next,
                        rhs_syntax: rhs_next,
                        parents: parents_next,
                        parents_hash,
                    },
                ));
                i += 1;
            }
        }

        if let (
            Syntax::Atom {
                content: lhs_content,
                kind: AtomKind::Comment,
                ..
            },
            Syntax::Atom {
                content: rhs_content,
                kind: AtomKind::Comment,
                ..
            },
        ) = (lhs_syntax, rhs_syntax)
        {
            // Both sides are comments and their content is reasonably
            // similar.
            if lhs_content != rhs_content {
                let levenshtein_pct =
                    (normalized_levenshtein(lhs_content, rhs_content) * 100.0).round() as u8;
                buf[i] = Some((
                    ReplacedComment { levenshtein_pct },
                    Vertex {
                        lhs_syntax: lhs_syntax.next_sibling(),
                        rhs_syntax: rhs_syntax.next_sibling(),
                        parents: v.parents.clone(),
                        parents_hash: v.parents_hash,
                    },
                ));
                i += 1;
            }
        }
    }

    if let Some(lhs_syntax) = &v.lhs_syntax {
        match lhs_syntax {
            // Step over this novel atom.
            Syntax::Atom { .. } => {
                buf[i] = Some((
                    NovelAtomLHS {
                        // TODO: should this apply if prev is a parent
                        // node rather than a sibling?
                        contiguous: lhs_syntax.prev_is_contiguous(),
                    },
                    Vertex {
                        lhs_syntax: lhs_syntax.next_sibling(),
                        rhs_syntax: v.rhs_syntax,
                        parents: v.parents.clone(),
                        parents_hash: v.parents_hash,
                    },
                ));
                i += 1;
            }
            // Step into this partially/fully novel list.
            Syntax::List {
                children,
                num_descendants,
                ..
            } => {
                let lhs_next = children.get(0).copied();

                let parents_next = push_lhs_delimiter(&v.parents, lhs_syntax);
                let parents_hash = hash_parents(&parents_next);

                buf[i] = Some((
                    EnterNovelDelimiterLHS {
                        contiguous: lhs_syntax.prev_is_contiguous(),
                    },
                    Vertex {
                        lhs_syntax: lhs_next,
                        rhs_syntax: v.rhs_syntax,
                        parents: parents_next,
                        parents_hash,
                    },
                ));
                i += 1;

                if *num_descendants > NOVEL_TREE_THRESHOLD && lhs_syntax.parent().is_none() {
                    buf[i] = Some((
                        NovelTreeLHS {
                            num_descendants: *num_descendants,
                        },
                        Vertex {
                            lhs_syntax: lhs_syntax.next_sibling(),
                            rhs_syntax: v.rhs_syntax,
                            parents: v.parents.clone(),
                            parents_hash: v.parents_hash,
                        },
                    ));
                    i += 1;
                }
            }
        }
    }

    if let Some(rhs_syntax) = &v.rhs_syntax {
        match rhs_syntax {
            // Step over this novel atom.
            Syntax::Atom { .. } => {
                buf[i] = Some((
                    NovelAtomRHS {
                        contiguous: rhs_syntax.prev_is_contiguous(),
                    },
                    Vertex {
                        lhs_syntax: v.lhs_syntax,
                        rhs_syntax: rhs_syntax.next_sibling(),
                        parents: v.parents.clone(),
                        parents_hash: v.parents_hash,
                    },
                ));
                i += 1;
            }
            // Step into this partially/fully novel list.
            Syntax::List {
                children,
                num_descendants,
                ..
            } => {
                let rhs_next = children.get(0).copied();

                let parents_next = push_rhs_delimiter(&v.parents, rhs_syntax);
                let parents_hash = hash_parents(&parents_next);

                buf[i] = Some((
                    EnterNovelDelimiterRHS {
                        contiguous: rhs_syntax.prev_is_contiguous(),
                    },
                    Vertex {
                        lhs_syntax: v.lhs_syntax,
                        rhs_syntax: rhs_next,
                        parents: parents_next,
                        parents_hash,
                    },
                ));
                i += 1;

                if *num_descendants > NOVEL_TREE_THRESHOLD && rhs_syntax.parent().is_none() {
                    buf[i] = Some((
                        NovelTreeRHS {
                            num_descendants: *num_descendants,
                        },
                        Vertex {
                            lhs_syntax: v.lhs_syntax,
                            rhs_syntax: rhs_syntax.next_sibling(),
                            parents: v.parents.clone(),
                            parents_hash: v.parents_hash,
                        },
                    ));
                    i += 1;
                }
            }
        }
    }
    assert!(
        i > 0,
        "Must always find some next steps if node is not the end"
    );
}

pub fn mark_route(route: &[(Edge, Vertex)]) {
    for (e, v) in route {
        match e {
            ExitDelimiterBoth | ExitDelimiterLHS | ExitDelimiterRHS => {
                // Nothing to do: we have already marked this node when we entered it.
            }
            UnchangedNode => {
                // No change on this node or its children.
                let lhs = v.lhs_syntax.unwrap();
                let rhs = v.rhs_syntax.unwrap();
                lhs.set_change_deep(ChangeKind::Unchanged(rhs));
                rhs.set_change_deep(ChangeKind::Unchanged(lhs));
            }
            EnterUnchangedDelimiter => {
                // No change on the outer delimiter, but children may
                // have changed.
                let lhs = v.lhs_syntax.unwrap();
                let rhs = v.rhs_syntax.unwrap();
                lhs.set_change(ChangeKind::Unchanged(rhs));
                rhs.set_change(ChangeKind::Unchanged(lhs));
            }
            ReplacedComment { levenshtein_pct } => {
                let lhs = v.lhs_syntax.unwrap();
                let rhs = v.rhs_syntax.unwrap();

                if *levenshtein_pct > 40 {
                    lhs.set_change(ChangeKind::ReplacedComment(lhs, rhs));
                    rhs.set_change(ChangeKind::ReplacedComment(rhs, lhs));
                } else {
                    lhs.set_change(ChangeKind::Novel);
                    rhs.set_change(ChangeKind::Novel);
                }
            }
            NovelAtomLHS { .. } | EnterNovelDelimiterLHS { .. } => {
                let lhs = v.lhs_syntax.unwrap();
                lhs.set_change(ChangeKind::Novel);
            }
            NovelAtomRHS { .. } | EnterNovelDelimiterRHS { .. } => {
                let rhs = v.rhs_syntax.unwrap();
                rhs.set_change(ChangeKind::Novel);
            }
            NovelTreeLHS { .. } => {
                let lhs = v.lhs_syntax.unwrap();
                lhs.set_change_deep(ChangeKind::Novel);
            }
            NovelTreeRHS { .. } => {
                let rhs = v.rhs_syntax.unwrap();
                rhs.set_change_deep(ChangeKind::Novel);
            }
        }
    }
}
