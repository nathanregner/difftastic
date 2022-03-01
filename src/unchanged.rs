use crate::syntax::{ChangeKind, Syntax};

#[derive(Debug)]
struct EqOnFirstItem<X, Y>(X, Y);

impl<X: Eq, Y> PartialEq for EqOnFirstItem<X, Y> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}
impl<X: Eq, Y> Eq for EqOnFirstItem<X, Y> {}

pub fn mark_unchanged_extract_possibly_changed<'a>(
    lhs_nodes: &[&'a Syntax<'a>],
    rhs_nodes: &[&'a Syntax<'a>],
) -> Vec<(Vec<&'a Syntax<'a>>, Vec<&'a Syntax<'a>>)> {
    let lhs_node_ids = lhs_nodes
        .iter()
        .map(|n| EqOnFirstItem(n.content_id(), *n))
        .collect::<Vec<_>>();
    let rhs_node_ids = rhs_nodes
        .iter()
        .map(|n| EqOnFirstItem(n.content_id(), *n))
        .collect::<Vec<_>>();

    let mut section_lhs_nodes = vec![];
    let mut section_rhs_nodes = vec![];
    let mut res = vec![];
    for diff_res in diff::slice(&lhs_node_ids, &rhs_node_ids) {
        match diff_res {
            diff::Result::Both(lhs, rhs) => {
                if !section_lhs_nodes.is_empty() || !section_rhs_nodes.is_empty() {
                    res.push((section_lhs_nodes, section_rhs_nodes));
                    section_lhs_nodes = vec![];
                    section_rhs_nodes = vec![];
                }

                let lhs_node = lhs.1;
                let rhs_node = rhs.1;

                lhs_node.set_change_deep(ChangeKind::Unchanged(rhs_node));
                rhs_node.set_change_deep(ChangeKind::Unchanged(lhs_node));
            }
            diff::Result::Left(lhs) => {
                section_lhs_nodes.push(lhs.1);
            }
            diff::Result::Right(rhs) => {
                section_rhs_nodes.push(rhs.1);
            }
        }
    }

    if !section_lhs_nodes.is_empty() || !section_rhs_nodes.is_empty() {
        res.push((section_lhs_nodes, section_rhs_nodes));
    }
    res
}

/// Discard nodes that are obviously unchanged, so we have a smaller
/// number of nodes to run the full diffing algorithm on.
pub fn skip_unchanged<'a>(
    lhs_nodes: &[&'a Syntax<'a>],
    rhs_nodes: &[&'a Syntax<'a>],
) -> (Vec<&'a Syntax<'a>>, Vec<&'a Syntax<'a>>) {
    let mut lhs_nodes = lhs_nodes.to_vec();
    let mut rhs_nodes = rhs_nodes.to_vec();

    // Repeatedly skip outer/leading/trailing nodes until we can't any
    // more (i.e. find a fixpoint).
    let mut keep_trying = true;
    while keep_trying {
        keep_trying = false;

        let (lhs_after_skip, rhs_after_skip) = skip_unchanged_at_ends(&lhs_nodes, &rhs_nodes);
        if lhs_after_skip != lhs_nodes {
            keep_trying = true;
            lhs_nodes = lhs_after_skip;
            rhs_nodes = rhs_after_skip;
        }

        let (lhs_after_skip, rhs_after_skip) = skip_unchanged_delimiters(&lhs_nodes, &rhs_nodes);
        if lhs_after_skip != lhs_nodes {
            keep_trying = true;
            lhs_nodes = lhs_after_skip;
            rhs_nodes = rhs_after_skip;
        }
    }

    (lhs_nodes, rhs_nodes)
}

/// If we're comparing two lists that have the same delimiters, mark
/// the delimiters as unchanged and return the children.
fn skip_unchanged_delimiters<'a>(
    lhs_nodes: &[&'a Syntax<'a>],
    rhs_nodes: &[&'a Syntax<'a>],
) -> (Vec<&'a Syntax<'a>>, Vec<&'a Syntax<'a>>) {
    if let (
        [Syntax::List {
            open_content: lhs_open,
            children: lhs_children,
            close_content: lhs_close,
            ..
        }],
        [Syntax::List {
            open_content: rhs_open,
            children: rhs_children,
            close_content: rhs_close,
            ..
        }],
    ) = (lhs_nodes, rhs_nodes)
    {
        if lhs_open == rhs_open && lhs_close == rhs_close {
            lhs_nodes[0].set_change(ChangeKind::Unchanged(rhs_nodes[0]));
            rhs_nodes[0].set_change(ChangeKind::Unchanged(lhs_nodes[0]));

            return (lhs_children.to_vec(), rhs_children.to_vec());
        }
    }

    (lhs_nodes.into(), rhs_nodes.into())
}

/// Skip syntax nodes at the beginning or end that are obviously
/// unchanged.
///
/// Set the ChangeKind on the definitely changed nodes, and return the
/// nodes that may contain changes.
fn skip_unchanged_at_ends<'a>(
    lhs_nodes: &[&'a Syntax<'a>],
    rhs_nodes: &[&'a Syntax<'a>],
) -> (Vec<&'a Syntax<'a>>, Vec<&'a Syntax<'a>>) {
    let mut lhs_nodes = lhs_nodes;
    let mut rhs_nodes = rhs_nodes;

    while let (Some(lhs_node), Some(rhs_node)) = (lhs_nodes.first(), rhs_nodes.first()) {
        if lhs_node.content_id() == rhs_node.content_id() {
            {
                lhs_node.set_change_deep(ChangeKind::Unchanged(rhs_node));
                rhs_node.set_change_deep(ChangeKind::Unchanged(lhs_node));
            };

            lhs_nodes = &lhs_nodes[1..];
            rhs_nodes = &rhs_nodes[1..];
        } else {
            break;
        }
    }

    while let (Some(lhs_node), Some(rhs_node)) = (lhs_nodes.last(), rhs_nodes.last()) {
        if lhs_node.content_id() == rhs_node.content_id() {
            lhs_node.set_change_deep(ChangeKind::Unchanged(rhs_node));
            rhs_node.set_change_deep(ChangeKind::Unchanged(lhs_node));

            lhs_nodes = &lhs_nodes[..lhs_nodes.len() - 1];
            rhs_nodes = &rhs_nodes[..rhs_nodes.len() - 1];
        } else {
            break;
        }
    }

    (Vec::from(lhs_nodes), Vec::from(rhs_nodes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        guess_language,
        syntax::init_all_info,
        tree_sitter_parser::{from_language, parse},
    };
    use pretty_assertions::assert_eq;
    use typed_arena::Arena;

    #[test]
    fn test_unchanged_at_start() {
        let arena = Arena::new();
        let config = from_language(guess_language::Language::EmacsLisp);

        let lhs_nodes = parse(&arena, "unchanged A B", &config);
        let rhs_nodes = parse(&arena, "unchanged X", &config);
        init_all_info(&lhs_nodes, &rhs_nodes);

        let (lhs_after_skip, rhs_after_skip) = skip_unchanged_at_ends(&lhs_nodes, &rhs_nodes);

        assert_eq!(
            lhs_nodes[0].change(),
            Some(ChangeKind::Unchanged(rhs_nodes[0]))
        );
        assert_eq!(
            rhs_nodes[0].change(),
            Some(ChangeKind::Unchanged(lhs_nodes[0]))
        );

        assert_eq!(lhs_after_skip.len(), 2);
        assert_eq!(rhs_after_skip.len(), 1);
    }

    #[test]
    fn test_unchanged_at_end() {
        let arena = Arena::new();
        let config = from_language(guess_language::Language::EmacsLisp);

        let lhs_nodes = parse(&arena, "A B unchanged", &config);
        let rhs_nodes = parse(&arena, "X unchanged", &config);
        init_all_info(&lhs_nodes, &rhs_nodes);

        let (lhs_after_skip, rhs_after_skip) = skip_unchanged_at_ends(&lhs_nodes, &rhs_nodes);

        assert_eq!(
            lhs_nodes[2].change(),
            Some(ChangeKind::Unchanged(rhs_nodes[1]))
        );
        assert_eq!(
            rhs_nodes[1].change(),
            Some(ChangeKind::Unchanged(lhs_nodes[2]))
        );

        assert_eq!(lhs_after_skip.len(), 2);
        assert_eq!(rhs_after_skip.len(), 1);
    }

    #[test]
    fn test_unchanged_outer_delimiters() {
        let arena = Arena::new();
        let config = from_language(guess_language::Language::EmacsLisp);

        let lhs_nodes = parse(&arena, "(A)", &config);
        let rhs_nodes = parse(&arena, "(B)", &config);
        init_all_info(&lhs_nodes, &rhs_nodes);

        let (lhs_after_skip, rhs_after_skip) = skip_unchanged_delimiters(&lhs_nodes, &rhs_nodes);

        // The only possibly changed nodes are inside the lists.
        assert_eq!(lhs_after_skip.len(), 1);
        assert!(matches!(lhs_after_skip[0], Syntax::Atom { .. }));

        assert_eq!(rhs_after_skip.len(), 1);
        assert!(matches!(rhs_after_skip[0], Syntax::Atom { .. }));

        // The outer list delimiters are unchanged.
        assert_eq!(
            lhs_nodes[0].change(),
            Some(ChangeKind::Unchanged(rhs_nodes[0]))
        );
        assert_eq!(
            rhs_nodes[0].change(),
            Some(ChangeKind::Unchanged(lhs_nodes[0]))
        );

        // The inner items haven't had their change set yet.
        assert_eq!(lhs_after_skip[0].change(), None);
        assert_eq!(rhs_after_skip[0].change(), None);
    }

    #[test]
    fn test_skip_unchanged() {
        let arena = Arena::new();
        let config = from_language(guess_language::Language::EmacsLisp);

        let lhs_nodes = parse(&arena, "unchanged-before (more-unchanged (A))", &config);
        let rhs_nodes = parse(&arena, "unchanged-before (more-unchanged (B))", &config);
        init_all_info(&lhs_nodes, &rhs_nodes);

        let (lhs_after_skip, rhs_after_skip) = skip_unchanged(&lhs_nodes, &rhs_nodes);

        // The only possibly changed nodes are inside the lists.
        assert_eq!(lhs_after_skip.len(), 1);
        assert!(matches!(lhs_after_skip[0], Syntax::Atom { .. }));

        assert_eq!(rhs_after_skip.len(), 1);
        assert!(matches!(rhs_after_skip[0], Syntax::Atom { .. }));

        // The inner items haven't had their change set yet.
        assert_eq!(lhs_after_skip[0].change(), None);
        assert_eq!(rhs_after_skip[0].change(), None);
    }

    #[test]
    fn test_unchanged_ids() {
        let arena = Arena::new();
        let config = from_language(guess_language::Language::EmacsLisp);

        let lhs_nodes = parse(&arena, "novel-lhs (unchanged ()) novel-lhs-2", &config);
        let rhs_nodes = parse(&arena, "novel-rhs (unchanged ()) novel-rhs-2", &config);
        init_all_info(&lhs_nodes, &rhs_nodes);

        mark_unchanged_extract_possibly_changed(&lhs_nodes, &rhs_nodes);

        assert_eq!(
            lhs_nodes[1].change(),
            Some(ChangeKind::Unchanged(rhs_nodes[1]))
        );
        assert_eq!(
            rhs_nodes[1].change(),
            Some(ChangeKind::Unchanged(lhs_nodes[1]))
        );
    }

    #[test]
    fn test_split_definitely_unchanged() {
        let arena = Arena::new();
        let config = from_language(guess_language::Language::EmacsLisp);

        let lhs_nodes = parse(&arena, "novel-lhs (unchanged ()) novel-lhs-2", &config);
        let rhs_nodes = parse(&arena, "novel-rhs (unchanged ()) novel-rhs-2", &config);
        init_all_info(&lhs_nodes, &rhs_nodes);

        let res = mark_unchanged_extract_possibly_changed(&lhs_nodes, &rhs_nodes);
        assert_eq!(
            res,
            vec![
                (vec![lhs_nodes[0]], vec![rhs_nodes[0]]),
                (vec![lhs_nodes[2]], vec![rhs_nodes[2]])
            ]
        );
    }
    #[test]
    fn test_split_definitely_unchanged_multiple() {
        let arena = Arena::new();
        let config = from_language(guess_language::Language::EmacsLisp);

        let lhs_nodes = parse(
            &arena,
            "novel-lhs unchanged-1 unchanged-2 novel-lhs-2",
            &config,
        );
        let rhs_nodes = parse(
            &arena,
            "novel-rhs unchanged-1 unchanged-2 novel-rhs-2",
            &config,
        );
        init_all_info(&lhs_nodes, &rhs_nodes);

        let res = mark_unchanged_extract_possibly_changed(&lhs_nodes, &rhs_nodes);
        assert_eq!(
            res,
            vec![
                (vec![lhs_nodes[0]], vec![rhs_nodes[0]]),
                (vec![lhs_nodes[3]], vec![rhs_nodes[3]])
            ]
        );
    }
}
