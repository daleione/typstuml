//! Shared helpers for the tree-shaped parsers (mind-map and WBS), which both
//! assemble a [`crate::ir::TreeNode`] via a depth stack.

use crate::ir::TreeNode;

/// Follow a child-index path from `root` and return the node it points at.
pub(crate) fn walk_mut<'a>(root: &'a mut TreeNode, path: &[usize]) -> &'a mut TreeNode {
    let mut cur = root;
    for &i in path {
        cur = &mut cur.children[i];
    }
    cur
}
