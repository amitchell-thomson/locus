//! Fallback threading by `References` / `In-Reply-To` (SPEC §7), for
//! messages without a source thread id. Subject-only grouping is off: two
//! "Hello" mails from different people stay separate.

use std::collections::HashMap;

/// A message's identity for threading.
pub struct Node<'a> {
    pub message_id: &'a str,
    pub in_reply_to: &'a [String],
    pub references: &'a [String],
}

/// One thread key per input message (the smallest message id in its group,
/// so keys are stable regardless of input order).
pub fn assign(nodes: &[Node<'_>]) -> Vec<String> {
    let mut parent: HashMap<String, String> = HashMap::new();
    fn find(p: &mut HashMap<String, String>, x: &str) -> String {
        let mut cur = x.to_string();
        let mut path = Vec::new();
        while let Some(next) = p.get(&cur).cloned() {
            if next == cur {
                break;
            }
            path.push(cur);
            cur = next;
        }
        for n in path {
            p.insert(n, cur.clone());
        }
        cur
    }
    fn union(p: &mut HashMap<String, String>, a: &str, b: &str) {
        let (ra, rb) = (find(p, a), find(p, b));
        if ra != rb {
            // The smaller id becomes the root: deterministic keys.
            let (lo, hi) = if ra < rb { (ra, rb) } else { (rb, ra) };
            p.insert(hi, lo);
        }
    }
    let key = |n: &Node<'_>, i: usize| -> String {
        if n.message_id.is_empty() {
            format!("\u{0}anon-{i}")
        } else {
            n.message_id.to_string()
        }
    };
    for (i, n) in nodes.iter().enumerate() {
        let k = key(n, i);
        parent.entry(k.clone()).or_insert_with(|| k.clone());
        for r in n.references.iter().chain(n.in_reply_to) {
            if r.is_empty() {
                continue;
            }
            parent.entry(r.clone()).or_insert_with(|| r.clone());
            union(&mut parent, &k, r);
        }
    }
    nodes
        .iter()
        .enumerate()
        .map(|(i, n)| find(&mut parent, &key(n, i)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n<'a>(id: &'a str, irt: &'a [String], refs: &'a [String]) -> Node<'a> {
        Node {
            message_id: id,
            in_reply_to: irt,
            references: refs,
        }
    }

    #[test]
    fn replies_join_their_thread() {
        let a_ref = vec!["a@x".to_string()];
        let b_ref = vec!["a@x".to_string(), "b@x".to_string()];
        let none: Vec<String> = vec![];
        let nodes = [
            n("c@x", &none, &b_ref),
            n("a@x", &none, &none),
            n("b@x", &a_ref, &a_ref),
        ];
        let k = assign(&nodes);
        assert!(k.iter().all(|x| x == "a@x"), "{k:?}");
    }

    #[test]
    fn same_subject_different_people_stay_apart() {
        let dir = format!("{}/../../fixtures/eml", env!("CARGO_MANIFEST_DIR"));
        let p = |f: &str| crate::mime::parse(&std::fs::read(format!("{dir}/{f}")).unwrap()).unwrap();
        let (h1, h2, a, b) = (
            p("21-hello-1.eml"),
            p("22-hello-2.eml"),
            p("19-thread-a.eml"),
            p("20-thread-b.eml"),
        );
        fn node(m: &crate::mime::Parsed) -> Node<'_> {
            Node {
                message_id: &m.message_id,
                in_reply_to: &m.in_reply_to,
                references: &m.references,
            }
        }
        let k = assign(&[node(&h1), node(&h2), node(&a), node(&b)]);
        assert_ne!(k[0], k[1], "two 'Hello' mails are different threads");
        assert_eq!(k[2], k[3], "a reply joins its thread");
    }

    #[test]
    fn reply_whose_parent_we_dont_have_still_groups_with_siblings() {
        let parent = vec!["missing@x".to_string()];
        let none: Vec<String> = vec![];
        let k = assign(&[
            n("r1@x", &parent, &none),
            n("r2@x", &parent, &none),
            n("other@x", &none, &none),
        ]);
        assert_eq!(k[0], k[1]);
        assert_ne!(k[0], k[2]);
    }

    #[test]
    fn missing_message_ids_dont_merge() {
        let none: Vec<String> = vec![];
        let k = assign(&[n("", &none, &none), n("", &none, &none)]);
        assert_ne!(k[0], k[1]);
    }
}
