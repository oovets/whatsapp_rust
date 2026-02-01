use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};
use serde::{Deserialize, Serialize};

use crate::widgets::ChatPane;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PaneNode {
    Single(usize),  // Index into App.panes
    Split {
        direction: SplitDirection,
        children: Vec<Box<PaneNode>>,
    },
}

impl PaneNode {
    pub fn new_single(pane_idx: usize) -> Self {
        PaneNode::Single(pane_idx)
    }

    pub fn split(&mut self, direction: SplitDirection, new_pane_idx: usize) {
        let old_node = std::mem::replace(self, PaneNode::Single(0));
        *self = PaneNode::Split {
            direction,
            children: vec![Box::new(old_node), Box::new(PaneNode::Single(new_pane_idx))],
        };
    }

    pub fn get_pane_indices(&self) -> Vec<usize> {
        match self {
            PaneNode::Single(idx) => vec![*idx],
            PaneNode::Split { children, .. } => {
                children.iter().flat_map(|child| child.get_pane_indices()).collect()
            }
        }
    }

    pub fn count_panes(&self) -> usize {
        match self {
            PaneNode::Single(_) => 1,
            PaneNode::Split { children, .. } => {
                children.iter().map(|child| child.count_panes()).sum()
            }
        }
    }

    pub fn find_and_remove_pane(&mut self, pane_idx: usize) -> bool {
        match self {
            PaneNode::Single(idx) => *idx == pane_idx,
            PaneNode::Split { children, .. } => {
                // Check if any child IS the pane we want to remove
                if let Some(pos) = children.iter().position(|child| {
                    matches!(**child, PaneNode::Single(idx) if idx == pane_idx)
                }) {
                    // Remove this direct child
                    children.remove(pos);
                    
                    // If only one child remains, collapse the split
                    if children.len() == 1 {
                        let child = children.remove(0);
                        *self = *child;
                    }
                    return true;
                }
                
                // Otherwise, recurse into children to find and remove
                for child in children.iter_mut() {
                    if child.find_and_remove_pane(pane_idx) {
                        return true;
                    }
                }
                
                false
            }
        }
    }

    pub fn render(
        &self,
        f: &mut Frame,
        area: Rect,
        panes: &[ChatPane],
        focused_idx: usize,
        render_fn: &impl Fn(&mut Frame, Rect, &ChatPane, bool),
        pane_areas: &mut std::collections::HashMap<usize, Rect>,
    ) {
        match self {
            PaneNode::Single(pane_idx) => {
                if let Some(pane) = panes.get(*pane_idx) {
                    let is_focused = *pane_idx == focused_idx;
                    pane_areas.insert(*pane_idx, area);
                    render_fn(f, area, pane, is_focused);
                }
            }
            PaneNode::Split { direction, children } => {
                if children.is_empty() {
                    return;
                }

                let constraints: Vec<Constraint> = (0..children.len())
                    .map(|_| Constraint::Percentage(100 / children.len() as u16))
                    .collect();

                let layout_direction = match direction {
                    SplitDirection::Horizontal => Direction::Vertical,
                    SplitDirection::Vertical => Direction::Horizontal,
                };

                let chunks = Layout::default()
                    .direction(layout_direction)
                    .constraints(constraints)
                    .split(area);

                for (i, child) in children.iter().enumerate() {
                    if let Some(&chunk) = chunks.get(i) {
                        child.render(f, chunk, panes, focused_idx, render_fn, pane_areas);
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub fn get_next_pane_idx(&self, current: usize) -> Option<usize> {
        let indices = self.get_pane_indices();
        if let Some(pos) = indices.iter().position(|&idx| idx == current) {
            let next_pos = (pos + 1) % indices.len();
            Some(indices[next_pos])
        } else {
            indices.first().copied()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_single_pane() {
        let mut node = PaneNode::new_single(0);
        node.split(SplitDirection::Vertical, 1);
        
        let indices = node.get_pane_indices();
        assert_eq!(indices, vec![0, 1]);
        assert_eq!(node.count_panes(), 2);
    }

    #[test]
    fn test_remove_pane() {
        let mut node = PaneNode::new_single(0);
        node.split(SplitDirection::Vertical, 1);
        
        let removed = node.find_and_remove_pane(1);
        assert!(removed);
        
        // Should collapse back to single
        match node {
            PaneNode::Single(idx) => assert_eq!(idx, 0),
            _ => panic!("Expected Single node after collapse"),
        }
    }

    #[test]
    fn test_cycle_focus() {
        let mut node = PaneNode::new_single(0);
        node.split(SplitDirection::Vertical, 1);
        node.split(SplitDirection::Horizontal, 2);
        
        let next = node.get_next_pane_idx(0);
        assert_eq!(next, Some(1));
        
        let next = node.get_next_pane_idx(2);
        assert_eq!(next, Some(0)); // Wraps around
    }
}
