//! Final gate status computation from individual validator results.
//!
//! Applies status suppression (`suppress_errors`, `suppress_warnings`) before
//! aggregating: Error beats Fail; Skip and Warn are ignored in the final roll-up.

use crate::types::*;

/// Computes the gate-level [`VerdictStatus`] from individual validator results,
/// applying status suppression. Error beats Fail; Skip and Warn are ignored.
pub fn compute_final_status(results: &[ValidatorResult], suppressed: &[Status]) -> VerdictStatus {
    let effective: Vec<Status> = results
        .iter()
        .filter(|r| r.status != Status::Skip)
        .map(|r| {
            if suppressed.contains(&r.status) {
                Status::Pass
            } else {
                r.status
            }
        })
        .collect();

    if effective.contains(&Status::Error) {
        VerdictStatus::Error
    } else if effective.contains(&Status::Fail) {
        VerdictStatus::Fail
    } else {
        VerdictStatus::Pass
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers as th;

    // ─── compute_final_status ────────────────────────

    #[test]
    fn final_status_all_pass() {
        let results = vec![th::result("a", Status::Pass), th::result("b", Status::Pass)];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Pass);
    }

    #[test]
    fn final_status_with_warn() {
        let results = vec![th::result("a", Status::Pass), th::result("b", Status::Warn)];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Pass);
    }

    #[test]
    fn final_status_with_fail() {
        let results = vec![th::result("a", Status::Pass), th::result("b", Status::Fail)];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Fail);
    }

    #[test]
    fn final_status_error_beats_fail() {
        let results = vec![
            th::result("a", Status::Fail),
            th::result("b", Status::Error),
        ];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Error);
    }

    #[test]
    fn final_status_skip_ignored() {
        let results = vec![th::result("a", Status::Skip), th::result("b", Status::Pass)];
        assert_eq!(compute_final_status(&results, &[]), VerdictStatus::Pass);
    }

    #[test]
    fn final_status_suppress_errors() {
        let results = vec![
            th::result("a", Status::Error),
            th::result("b", Status::Fail),
        ];
        assert_eq!(
            compute_final_status(&results, &[Status::Error]),
            VerdictStatus::Fail
        );
    }

    #[test]
    fn final_status_suppress_all() {
        let results = vec![
            th::result("a", Status::Error),
            th::result("b", Status::Fail),
        ];
        assert_eq!(
            compute_final_status(&results, &[Status::Error, Status::Fail, Status::Warn]),
            VerdictStatus::Pass
        );
    }

    #[test]
    fn compute_final_status_empty_results_is_pass() {
        assert_eq!(compute_final_status(&[], &[]), VerdictStatus::Pass);
    }
}
