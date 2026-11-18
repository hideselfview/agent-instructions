//! Sequence alignment of the two reading orders: the longest common
//! subsequence of their (kind, words) keys.

use crate::shots::Role;

pub(crate) type Key = (Role, String);

/// One step of the alignment, with indices into the board's keys and the
/// shot's keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Step {
    Match(usize, usize),
    /// Only the board has it.
    Board(usize),
    /// Only the shot has it.
    Shot(usize),
}

/// The steps that carry the board's keys into the shot's, in reading order.
pub(crate) fn align(board: &[Key], shot: &[Key]) -> Vec<Step> {
    let (n, m) = (board.len(), shot.len());
    // common[i][j]: the longest common subsequence of board[i..] and shot[j..].
    let mut common = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            common[i][j] = if board[i] == shot[j] {
                common[i + 1][j + 1] + 1
            } else {
                common[i + 1][j].max(common[i][j + 1])
            };
        }
    }
    let mut steps = Vec::with_capacity(n.max(m));
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && board[i] == shot[j] {
            steps.push(Step::Match(i, j));
            i += 1;
            j += 1;
        } else if j == m || (i < n && common[i + 1][j] >= common[i][j + 1]) {
            steps.push(Step::Board(i));
            i += 1;
        } else {
            steps.push(Step::Shot(j));
            j += 1;
        }
    }
    steps
}

#[cfg(test)]
#[path = "align_tests.rs"]
mod tests;
