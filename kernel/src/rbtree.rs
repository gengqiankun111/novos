//! 固定节点池红黑树（M2-切片4：CFS runqueue）。
//!
//! - **IRQ 上下文安全**：节点池为静态数组，无堆分配（调度器在 tick 中断中操作树，
//!   若走 `alloc` 会与被打断任务持有的分配器锁死锁）；
//! - 节点按**任务 id 索引**（`nodes[id]` 即任务 id 的节点，id < MAX_RB_NODES）；
//! - 用真实哨兵节点（`SENTINEL`）实现 CLRS 标准红黑树，nil 有合法 parent 字段，
//!   `delete_fixup` 可照搬教科书实现。
//!
//! 对应 DESIGN.md §4.2：runqueue = 侵入式红黑树（此处为固定池数组版），取最左 = 最小 vruntime。

/// 任务节点槽数（须 ≥ MAX_TASKS）。
pub const MAX_RB_NODES: usize = 16;
/// 哨兵节点索引（数组最后一个槽）。
const SENTINEL: usize = MAX_RB_NODES;

const RED: u8 = 0;
const BLACK: u8 = 1;

#[derive(Clone, Copy)]
struct Node {
    key: u64,
    parent: usize,
    left: usize,
    right: usize,
    color: u8,
}

const fn node() -> Node {
    Node { key: 0, parent: SENTINEL, left: SENTINEL, right: SENTINEL, color: BLACK }
}

pub struct RbTree {
    nodes: [Node; MAX_RB_NODES + 1],
    root: usize,
    count: usize,
}

impl RbTree {
    pub const fn new() -> Self {
        RbTree {
            nodes: [node(); MAX_RB_NODES + 1],
            root: SENTINEL,
            count: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.root == SENTINEL
    }

    /// 最小 key 节点对应的任务 id；空树返回 None。
    pub fn min(&self) -> Option<usize> {
        if self.root == SENTINEL {
            return None;
        }
        let mut x = self.root;
        while self.nodes[x].left != SENTINEL {
            x = self.nodes[x].left;
        }
        Some(x)
    }

    fn rotate_left(&mut self, x: usize) {
        let y = self.nodes[x].right;
        // x.right = y.left
        self.nodes[x].right = self.nodes[y].left;
        if self.nodes[y].left != SENTINEL {
            self.nodes[self.nodes[y].left].parent = x;
        }
        // y.parent = x.parent
        self.nodes[y].parent = self.nodes[x].parent;
        if self.nodes[x].parent == SENTINEL {
            self.root = y;
        } else if x == self.nodes[self.nodes[x].parent].left {
            self.nodes[self.nodes[x].parent].left = y;
        } else {
            self.nodes[self.nodes[x].parent].right = y;
        }
        // y.left = x
        self.nodes[y].left = x;
        self.nodes[x].parent = y;
    }

    fn rotate_right(&mut self, x: usize) {
        let y = self.nodes[x].left;
        self.nodes[x].left = self.nodes[y].right;
        if self.nodes[y].right != SENTINEL {
            self.nodes[self.nodes[y].right].parent = x;
        }
        self.nodes[y].parent = self.nodes[x].parent;
        if self.nodes[x].parent == SENTINEL {
            self.root = y;
        } else if x == self.nodes[self.nodes[x].parent].left {
            self.nodes[self.nodes[x].parent].left = y;
        } else {
            self.nodes[self.nodes[x].parent].right = y;
        }
        self.nodes[y].right = x;
        self.nodes[x].parent = y;
    }

    /// 插入任务 `id`（key = vruntime）。调用方须保证 id 不在树中。
    pub fn insert(&mut self, id: usize, key: u64) {
        debug_assert!(id < MAX_RB_NODES);
        let z = id;
        self.nodes[z].key = key;
        let mut y = SENTINEL;
        let mut x = self.root;
        while x != SENTINEL {
            y = x;
            x = if key < self.nodes[x].key {
                self.nodes[x].left
            } else {
                self.nodes[x].right
            };
        }
        self.nodes[z].parent = y;
        if y == SENTINEL {
            self.root = z;
        } else if key < self.nodes[y].key {
            self.nodes[y].left = z;
        } else {
            self.nodes[y].right = z;
        }
        self.nodes[z].left = SENTINEL;
        self.nodes[z].right = SENTINEL;
        self.nodes[z].color = RED;
        self.insert_fixup(z);
        self.count += 1;
    }

    fn insert_fixup(&mut self, mut z: usize) {
        while self.nodes[self.nodes[z].parent].color == RED {
            let p = self.nodes[z].parent;
            let g = self.nodes[p].parent;
            if p == self.nodes[g].left {
                let y = self.nodes[g].right;
                if self.nodes[y].color == RED {
                    self.nodes[p].color = BLACK;
                    self.nodes[y].color = BLACK;
                    self.nodes[g].color = RED;
                    z = g;
                } else {
                    if z == self.nodes[p].right {
                        z = p;
                        self.rotate_left(z);
                    }
                    let p2 = self.nodes[z].parent;
                    let g2 = self.nodes[p2].parent;
                    self.nodes[p2].color = BLACK;
                    self.nodes[g2].color = RED;
                    self.rotate_right(g2);
                }
            } else {
                let y = self.nodes[g].left;
                if self.nodes[y].color == RED {
                    self.nodes[p].color = BLACK;
                    self.nodes[y].color = BLACK;
                    self.nodes[g].color = RED;
                    z = g;
                } else {
                    if z == self.nodes[p].left {
                        z = p;
                        self.rotate_right(z);
                    }
                    let p2 = self.nodes[z].parent;
                    let g2 = self.nodes[p2].parent;
                    self.nodes[p2].color = BLACK;
                    self.nodes[g2].color = RED;
                    self.rotate_left(g2);
                }
            }
        }
        self.nodes[self.root].color = BLACK;
    }

    fn transplant(&mut self, u: usize, v: usize) {
        if self.nodes[u].parent == SENTINEL {
            self.root = v;
        } else if u == self.nodes[self.nodes[u].parent].left {
            self.nodes[self.nodes[u].parent].left = v;
        } else {
            self.nodes[self.nodes[u].parent].right = v;
        }
        self.nodes[v].parent = self.nodes[u].parent;
    }

    fn tree_min(&self, mut x: usize) -> usize {
        while self.nodes[x].left != SENTINEL {
            x = self.nodes[x].left;
        }
        x
    }

    /// 删除任务 `id` 的节点。调用方须保证 id 在树中。
    pub fn remove(&mut self, id: usize) {
        debug_assert!(id < MAX_RB_NODES);
        let z = id;
        let mut x;
        let mut y = z;
        let y_orig_color = self.nodes[y].color;
        if self.nodes[z].left == SENTINEL {
            x = self.nodes[z].right;
            self.transplant(z, self.nodes[z].right);
        } else if self.nodes[z].right == SENTINEL {
            x = self.nodes[z].left;
            self.transplant(z, self.nodes[z].left);
        } else {
            y = self.tree_min(self.nodes[z].right);
            let y_orig_color2 = self.nodes[y].color;
            x = self.nodes[y].right;
            if self.nodes[y].parent == z {
                self.nodes[x].parent = y;
            } else {
                self.transplant(y, self.nodes[y].right);
                self.nodes[y].right = self.nodes[z].right;
                self.nodes[self.nodes[y].right].parent = y;
            }
            self.transplant(z, y);
            self.nodes[y].left = self.nodes[z].left;
            self.nodes[self.nodes[y].left].parent = y;
            self.nodes[y].color = self.nodes[z].color;
            if y_orig_color2 == BLACK {
                self.delete_fixup(x);
            }
            self.count -= 1;
            return;
        }
        if y_orig_color == BLACK {
            self.delete_fixup(x);
        }
        self.count -= 1;
    }

    fn delete_fixup(&mut self, mut x: usize) {
        while x != self.root && self.nodes[x].color == BLACK {
            if x == self.nodes[self.nodes[x].parent].left {
                let mut w = self.nodes[self.nodes[x].parent].right;
                if self.nodes[w].color == RED {
                    self.nodes[w].color = BLACK;
                    self.nodes[self.nodes[x].parent].color = RED;
                    self.rotate_left(self.nodes[x].parent);
                    w = self.nodes[self.nodes[x].parent].right;
                }
                if self.nodes[self.nodes[w].left].color == BLACK
                    && self.nodes[self.nodes[w].right].color == BLACK
                {
                    self.nodes[w].color = RED;
                    x = self.nodes[x].parent;
                } else {
                    if self.nodes[self.nodes[w].right].color == BLACK {
                        self.nodes[self.nodes[w].left].color = BLACK;
                        self.nodes[w].color = RED;
                        self.rotate_right(w);
                        w = self.nodes[self.nodes[x].parent].right;
                    }
                    self.nodes[w].color = self.nodes[self.nodes[x].parent].color;
                    self.nodes[self.nodes[x].parent].color = BLACK;
                    self.nodes[self.nodes[w].right].color = BLACK;
                    self.rotate_left(self.nodes[x].parent);
                    x = self.root;
                }
            } else {
                let mut w = self.nodes[self.nodes[x].parent].left;
                if self.nodes[w].color == RED {
                    self.nodes[w].color = BLACK;
                    self.nodes[self.nodes[x].parent].color = RED;
                    self.rotate_right(self.nodes[x].parent);
                    w = self.nodes[self.nodes[x].parent].left;
                }
                if self.nodes[self.nodes[w].right].color == BLACK
                    && self.nodes[self.nodes[w].left].color == BLACK
                {
                    self.nodes[w].color = RED;
                    x = self.nodes[x].parent;
                } else {
                    if self.nodes[self.nodes[w].left].color == BLACK {
                        self.nodes[self.nodes[w].right].color = BLACK;
                        self.nodes[w].color = RED;
                        self.rotate_left(w);
                        w = self.nodes[self.nodes[x].parent].left;
                    }
                    self.nodes[w].color = self.nodes[self.nodes[x].parent].color;
                    self.nodes[self.nodes[x].parent].color = BLACK;
                    self.nodes[self.nodes[w].left].color = BLACK;
                    self.rotate_right(self.nodes[x].parent);
                    x = self.root;
                }
            }
        }
        self.nodes[x].color = BLACK;
    }
}

// ---- 自测（host 可跑：`cargo test` 用）----

/// 校验红黑树性质 + 中序遍历有序（调试/测试）。
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    fn in_order(t: &RbTree, x: usize, out: &mut Vec<u64>) {
        if x == SENTINEL {
            return;
        }
        in_order(t, t.nodes[x].left, out);
        out.push(t.nodes[x].key);
        in_order(t, t.nodes[x].right, out);
    }

    fn black_height(t: &RbTree, x: usize) -> usize {
        if x == SENTINEL {
            return 1;
        }
        let l = black_height(t, t.nodes[x].left);
        let r = black_height(t, t.nodes[x].right);
        assert_eq!(l, r, "黑高不平衡 at node {x}");
        l + (t.nodes[x].color == BLACK) as usize
    }

    #[test]
    fn rbt_insert_remove_random() {
        let mut t = RbTree::new();
        // 插入 8 个不同 key（随机顺序）
        let keys = [50, 30, 70, 20, 40, 60, 80, 10];
        for (i, k) in keys.iter().enumerate() {
            t.insert(i, *k);
        }
        assert_eq!(t.count, 8);
        // 中序应有序
        let mut v = Vec::new();
        in_order(&t, t.root, &mut v);
        assert_eq!(v, vec![10, 20, 30, 40, 50, 60, 70, 80]);
        // 黑高一致
        black_height(&t, t.root);
        // 最小 = 10
        assert_eq!(t.min(), Some(7));
        // 删除最左，逐步删除（节点 7/3/2 的 key 分别为 10/20/70）
        t.remove(7); // key 10
        t.remove(3); // key 20
        t.remove(2); // key 70
        black_height(&t, t.root);
        assert_eq!(t.min(), Some(1)); // key 30
        // 全部删除
        for i in [4, 1, 5, 6, 0] {
            t.remove(i);
        }
        assert!(t.is_empty());
    }

    #[test]
    fn empty_tree_queries() {
        let t = RbTree::new();
        assert!(t.is_empty());
        assert_eq!(t.min(), None);
        assert_eq!(t.count, 0);
    }

    #[test]
    fn single_node_roundtrip() {
        let mut t = RbTree::new();
        t.insert(0, 100);
        assert!(!t.is_empty());
        assert_eq!(t.count, 1);
        assert_eq!(t.min(), Some(0));
        t.remove(0);
        assert!(t.is_empty());
        assert_eq!(t.min(), None);
        assert_eq!(t.count, 0);
    }

    #[test]
    fn duplicate_keys_both_retained() {
        let mut t = RbTree::new();
        t.insert(0, 5);
        t.insert(1, 5);
        t.insert(2, 9);
        assert_eq!(t.count, 3);
        // 等键走右分支：min 恒为第一个插入者（id 0）
        assert_eq!(t.min(), Some(0));
        t.remove(0);
        assert_eq!(t.min(), Some(1));
        t.remove(1);
        assert_eq!(t.min(), Some(2));
        t.remove(2);
        assert!(t.is_empty());
    }

    #[test]
    fn extreme_keys_min() {
        let mut t = RbTree::new();
        t.insert(0, u64::MAX);
        t.insert(1, 0); // u64::MIN
        t.insert(2, u64::MAX / 2);
        assert_eq!(t.count, 3);
        assert_eq!(t.min(), Some(1));
        black_height(&t, t.root);
        t.remove(1);
        assert_eq!(t.min(), Some(2)); // 剩 MAX/2 与 MAX
        black_height(&t, t.root);
    }

    #[test]
    fn full_pool_roundtrip() {
        let mut t = RbTree::new();
        // 填满 16 槽，key 乱序
        let keys = [4u64, 9, 16, 25, 36, 49, 64, 81, 100, 121, 144, 169, 196, 225, 256, 1];
        for (i, k) in keys.iter().enumerate() {
            t.insert(i, *k);
        }
        assert_eq!(t.count, MAX_RB_NODES);
        black_height(&t, t.root);
        assert_eq!(t.min(), Some(15)); // key 1 最小
        for i in 0..MAX_RB_NODES {
            t.remove(i);
        }
        assert!(t.is_empty());
        assert_eq!(t.count, 0);
    }

    #[test]
    fn remove_min_repeatedly_ascends() {
        let mut t = RbTree::new();
        let keys = [50u64, 30, 70, 20, 40, 60, 80, 10];
        for (i, k) in keys.iter().enumerate() {
            t.insert(i, *k);
        }
        // key 升序 → 节点：10→7, 20→3, 30→1, 40→4, 50→0, 60→5, 70→2, 80→6
        for expect in [7, 3, 1, 4, 0, 5, 2, 6] {
            assert_eq!(t.min(), Some(expect), "删除后 min 应逐级上升");
            t.remove(expect);
        }
        assert!(t.is_empty());
    }

    #[test]
    fn min_untouched_by_removing_largest() {
        let mut t = RbTree::new();
        let keys = [50u64, 30, 70, 20, 40];
        for (i, k) in keys.iter().enumerate() {
            t.insert(i, *k);
        }
        t.remove(0); // key 50
        t.remove(2); // key 70
        assert_eq!(t.min(), Some(3)); // key 20 不受影响
        black_height(&t, t.root);
        t.remove(3);
        assert_eq!(t.min(), Some(1)); // key 30
    }
}
