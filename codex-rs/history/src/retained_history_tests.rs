use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use super::*;

fn class_budget(max_items: usize, max_charged_bytes: usize) -> RetainedHistoryClassBudget {
    RetainedHistoryClassBudget::new(max_items, max_charged_bytes)
}

fn budget(
    protected: RetainedHistoryClassBudget,
    ordinary: RetainedHistoryClassBudget,
) -> RetainedHistoryBudget {
    RetainedHistoryBudget::new(protected, ordinary)
}

fn labels(entries: &[RetainedHistoryEntry<String>]) -> Vec<&str> {
    entries
        .iter()
        .map(|entry| entry.payload().as_str())
        .collect()
}

fn append_string(
    history: &mut RetainedHistory<String>,
    class: RetainedHistoryClass,
    charged_bytes: usize,
    value: &str,
) -> RetainedHistoryAppendOutcome {
    history.append(class, charged_bytes, &value.to_string())
}

#[test]
fn retained_entries_keep_original_cross_class_chronological_order() {
    let mut history = RetainedHistory::new(budget(class_budget(10, 100), class_budget(10, 100)));

    append_string(&mut history, RetainedHistoryClass::Protected, 1, "p1");
    append_string(&mut history, RetainedHistoryClass::Ordinary, 1, "o1");
    append_string(&mut history, RetainedHistoryClass::Protected, 1, "p2");
    append_string(&mut history, RetainedHistoryClass::Ordinary, 1, "o2");

    assert_eq!(labels(history.entries()), vec!["p1", "o1", "p2", "o2"]);
    assert_eq!(
        history.entries()[0].class(),
        RetainedHistoryClass::Protected
    );
    assert_eq!(history.entries()[1].charged_bytes(), 1);
}

#[test]
fn count_and_byte_overflow_evict_only_from_the_class_that_overflowed() {
    let mut history = RetainedHistory::new(budget(class_budget(2, 4), class_budget(2, 4)));

    append_string(&mut history, RetainedHistoryClass::Protected, 2, "p1");
    append_string(&mut history, RetainedHistoryClass::Ordinary, 2, "o1");
    append_string(&mut history, RetainedHistoryClass::Protected, 2, "p2");
    append_string(&mut history, RetainedHistoryClass::Ordinary, 2, "o2");
    assert_eq!(history.generation(), 0);

    let protected_overflow = append_string(&mut history, RetainedHistoryClass::Protected, 1, "p3");
    assert_eq!(
        protected_overflow,
        RetainedHistoryAppendOutcome::Appended {
            evicted: RetainedHistoryClassUsage {
                item_count: 1,
                charged_bytes: 2,
            },
        }
    );
    assert_eq!(history.generation(), 1);
    assert_eq!(
        history.accounting(),
        RetainedHistoryUsage {
            protected: RetainedHistoryClassUsage {
                item_count: 2,
                charged_bytes: 3,
            },
            ordinary: RetainedHistoryClassUsage {
                item_count: 2,
                charged_bytes: 4,
            },
        }
    );

    let ordinary_overflow = append_string(&mut history, RetainedHistoryClass::Ordinary, 1, "o3");
    assert_eq!(
        ordinary_overflow,
        RetainedHistoryAppendOutcome::Appended {
            evicted: RetainedHistoryClassUsage {
                item_count: 1,
                charged_bytes: 2,
            },
        }
    );
    assert_eq!(history.generation(), 2);
    assert_eq!(labels(history.entries()), vec!["p2", "o2", "p3", "o3"]);
    assert_eq!(
        history.accounting(),
        RetainedHistoryUsage {
            protected: RetainedHistoryClassUsage {
                item_count: 2,
                charged_bytes: 3,
            },
            ordinary: RetainedHistoryClassUsage {
                item_count: 2,
                charged_bytes: 3,
            },
        }
    );
}

#[test]
fn byte_only_overflow_evicts_minimum_protected_and_half_plus_enough_ordinary() {
    let mut protected = RetainedHistory::new(budget(class_budget(10, 10), class_budget(0, 0)));
    append_string(&mut protected, RetainedHistoryClass::Protected, 3, "p1");
    append_string(&mut protected, RetainedHistoryClass::Protected, 4, "p2");
    append_string(&mut protected, RetainedHistoryClass::Protected, 3, "p3");

    let protected_outcome = append_string(&mut protected, RetainedHistoryClass::Protected, 3, "p4");

    assert_eq!(
        protected_outcome,
        RetainedHistoryAppendOutcome::Appended {
            evicted: RetainedHistoryClassUsage {
                item_count: 1,
                charged_bytes: 3,
            },
        }
    );
    assert_eq!(labels(protected.entries()), vec!["p2", "p3", "p4"]);

    let mut ordinary = RetainedHistory::new(budget(class_budget(10, 100), class_budget(10, 10)));
    append_string(
        &mut ordinary,
        RetainedHistoryClass::Protected,
        1,
        "protected",
    );
    append_string(&mut ordinary, RetainedHistoryClass::Ordinary, 2, "o1");
    append_string(&mut ordinary, RetainedHistoryClass::Ordinary, 2, "o2");
    append_string(&mut ordinary, RetainedHistoryClass::Ordinary, 2, "o3");
    append_string(&mut ordinary, RetainedHistoryClass::Ordinary, 4, "o4");

    let ordinary_outcome = append_string(&mut ordinary, RetainedHistoryClass::Ordinary, 5, "o5");

    assert_eq!(
        ordinary_outcome,
        RetainedHistoryAppendOutcome::Appended {
            evicted: RetainedHistoryClassUsage {
                item_count: 3,
                charged_bytes: 6,
            },
        }
    );
    assert_eq!(labels(ordinary.entries()), vec!["protected", "o4", "o5"]);
    assert_eq!(ordinary.accounting().protected.item_count, 1);
}

#[test]
fn ordinary_overflow_evicts_at_least_half_of_existing_ordinary_entries() {
    let mut history = RetainedHistory::new(budget(class_budget(10, 100), class_budget(3, 100)));
    append_string(&mut history, RetainedHistoryClass::Protected, 1, "p1");
    append_string(&mut history, RetainedHistoryClass::Ordinary, 1, "o1");
    append_string(&mut history, RetainedHistoryClass::Ordinary, 1, "o2");
    append_string(&mut history, RetainedHistoryClass::Ordinary, 1, "o3");

    let outcome = append_string(&mut history, RetainedHistoryClass::Ordinary, 1, "o4");

    assert_eq!(
        outcome,
        RetainedHistoryAppendOutcome::Appended {
            evicted: RetainedHistoryClassUsage {
                item_count: 2,
                charged_bytes: 2,
            },
        }
    );
    assert_eq!(labels(history.entries()), vec!["p1", "o3", "o4"]);
    assert_eq!(
        history.accounting().protected,
        RetainedHistoryClassUsage {
            item_count: 1,
            charged_bytes: 1,
        }
    );
}

#[test]
fn rejection_on_a_full_nonempty_history_preserves_state_and_snapshot_without_cloning() {
    let clones = Arc::new(AtomicUsize::new(0));
    let mut history = RetainedHistory::new(budget(class_budget(1, 1), class_budget(0, 0)));
    let retained = CountedValue::new("retained", &clones);
    let rejected = CountedValue::new("rejected", &clones);
    history.append(RetainedHistoryClass::Protected, 1, &retained);
    let snapshot = history.snapshot();
    let before_accounting = history.accounting();
    let before_generation = history.generation();
    assert_eq!(clones.load(Ordering::SeqCst), 1);

    assert_eq!(
        history.append(RetainedHistoryClass::Protected, 2, &rejected),
        RetainedHistoryAppendOutcome::Rejected(RetainedHistoryRejection::EntryExceedsByteLimit {
            class: RetainedHistoryClass::Protected,
            charged_bytes: 2,
            max_charged_bytes: 1,
        })
    );

    assert_eq!(history.entries().len(), 1);
    assert_eq!(history.entries()[0].payload().label, "retained");
    assert_eq!(snapshot.entries().len(), 1);
    assert_eq!(snapshot.entries()[0].payload().label, "retained");
    assert_eq!(history.accounting(), before_accounting);
    assert_eq!(history.generation(), before_generation);
    assert_eq!(clones.load(Ordering::SeqCst), 1);
}

#[derive(Debug)]
struct CountedValue {
    label: &'static str,
    clone_count: Arc<AtomicUsize>,
}

impl CountedValue {
    fn new(label: &'static str, clone_count: &Arc<AtomicUsize>) -> Self {
        Self {
            label,
            clone_count: Arc::clone(clone_count),
        }
    }
}

impl Clone for CountedValue {
    fn clone(&self) -> Self {
        self.clone_count.fetch_add(1, Ordering::SeqCst);
        Self {
            label: self.label,
            clone_count: Arc::clone(&self.clone_count),
        }
    }
}

#[test]
fn disabled_and_oversized_entries_are_rejected_before_payload_clone() {
    let clones = Arc::new(AtomicUsize::new(0));
    let payload = CountedValue::new("item", &clones);
    let mut history = RetainedHistory::new(budget(class_budget(0, 100), class_budget(5, 4)));

    assert_eq!(
        history.append(RetainedHistoryClass::Protected, 1, &payload),
        RetainedHistoryAppendOutcome::Rejected(RetainedHistoryRejection::ClassDisabled {
            class: RetainedHistoryClass::Protected,
        })
    );
    assert_eq!(
        history.append(RetainedHistoryClass::Ordinary, 5, &payload),
        RetainedHistoryAppendOutcome::Rejected(RetainedHistoryRejection::EntryExceedsByteLimit {
            class: RetainedHistoryClass::Ordinary,
            charged_bytes: 5,
            max_charged_bytes: 4,
        })
    );
    assert_eq!(clones.load(Ordering::SeqCst), 0);

    let mut zero_byte_budget =
        RetainedHistory::new(budget(class_budget(5, 100), class_budget(5, 0)));
    assert_eq!(
        zero_byte_budget.append(RetainedHistoryClass::Ordinary, 0, &payload),
        RetainedHistoryAppendOutcome::Rejected(RetainedHistoryRejection::ClassDisabled {
            class: RetainedHistoryClass::Ordinary,
        })
    );
    assert_eq!(clones.load(Ordering::SeqCst), 0);
}

#[test]
fn checked_byte_accounting_handles_sum_overflow_by_evicting_before_append() {
    let mut history = RetainedHistory::new(budget(class_budget(2, usize::MAX), class_budget(0, 0)));
    append_string(
        &mut history,
        RetainedHistoryClass::Protected,
        usize::MAX - 8,
        "large-old",
    );

    let outcome = append_string(&mut history, RetainedHistoryClass::Protected, 16, "new");

    assert_eq!(
        outcome,
        RetainedHistoryAppendOutcome::Appended {
            evicted: RetainedHistoryClassUsage {
                item_count: 1,
                charged_bytes: usize::MAX - 8,
            },
        }
    );
    assert_eq!(labels(history.entries()), ["new"]);
    assert_eq!(history.accounting().protected.charged_bytes, 16);
}

#[test]
fn snapshots_and_history_clones_keep_old_prefixes_across_append_reset_and_truncate() {
    let clones = Arc::new(AtomicUsize::new(0));
    let mut history = RetainedHistory::new(budget(class_budget(10, 100), class_budget(10, 100)));
    let first = CountedValue::new("first", &clones);
    let second = CountedValue::new("second", &clones);
    let replacement = CountedValue::new("replacement", &clones);
    let tail = CountedValue::new("tail", &clones);

    assert_eq!(history.generation(), 0);
    assert!(matches!(
        history.append(RetainedHistoryClass::Protected, 1, &first),
        RetainedHistoryAppendOutcome::Appended {
            evicted: RetainedHistoryClassUsage {
                item_count: 0,
                charged_bytes: 0
            }
        }
    ));
    let first_snapshot = history.snapshot();
    let history_clone = history.clone();
    let snapshot_clone = first_snapshot.clone();
    assert_eq!(clones.load(Ordering::SeqCst), 1);
    assert_eq!(history.generation(), 0);

    history.append(RetainedHistoryClass::Protected, 1, &second);
    let pre_reset_snapshot = history.snapshot();
    assert_eq!(clones.load(Ordering::SeqCst), 2);
    assert_eq!(first_snapshot.entries().len(), 1);
    assert_eq!(first_snapshot.entries()[0].payload().label, "first");
    assert_eq!(first_snapshot.accounting().protected.item_count, 1);
    assert_eq!(history_clone.entries()[0].payload().label, "first");
    assert_eq!(snapshot_clone.generation(), 0);

    let reset = history.reset([(RetainedHistoryClass::Protected, 1, &replacement)]);
    assert_eq!(reset.cleared.protected.item_count, 2);
    assert_eq!(reset.admitted_items, 1);
    assert_eq!(
        reset.rejected,
        RetainedHistoryResetRejectionCounts::default()
    );
    assert_eq!(history.generation(), 1);
    assert_eq!(clones.load(Ordering::SeqCst), 3);
    let reset_snapshot = history.snapshot();

    history.append(RetainedHistoryClass::Protected, 1, &tail);
    let pre_truncate_snapshot = history.snapshot();
    assert_eq!(history.generation(), 1);
    assert_eq!(clones.load(Ordering::SeqCst), 4);

    let truncation = history.truncate_before(|entry| entry.payload().label == "tail");
    assert!(truncation.boundary_found);
    assert_eq!(truncation.removed.protected.item_count, 1);
    assert_eq!(history.entries().len(), 1);
    assert_eq!(history.entries()[0].payload().label, "replacement");
    assert_eq!(history.generation(), 2);
    assert_eq!(reset_snapshot.entries().len(), 1);
    assert_eq!(reset_snapshot.entries()[0].payload().label, "replacement");
    assert_eq!(pre_truncate_snapshot.entries().len(), 2);
    assert_eq!(pre_reset_snapshot.entries().len(), 2);
    assert_eq!(clones.load(Ordering::SeqCst), 4);
}

#[test]
fn history_and_snapshot_cloning_do_not_require_payload_clone() {
    struct NonClone;

    let history = RetainedHistory::<NonClone>::new(budget(class_budget(1, 1), class_budget(1, 1)));
    let cloned_history = history.clone();
    let snapshot = cloned_history.snapshot();
    let cloned_snapshot = snapshot.clone();

    assert!(cloned_snapshot.entries().is_empty());
    assert_eq!(cloned_snapshot.generation(), 0);
}

#[test]
fn unknown_truncation_boundary_clears_all_and_advances_generation() {
    let mut history = RetainedHistory::new(budget(class_budget(10, 100), class_budget(10, 100)));
    append_string(&mut history, RetainedHistoryClass::Ordinary, 1, "one");
    append_string(&mut history, RetainedHistoryClass::Protected, 2, "two");
    let old_snapshot = history.snapshot();

    let outcome = history.truncate_before(|_| false);

    assert!(!outcome.boundary_found);
    assert_eq!(
        outcome.removed,
        RetainedHistoryUsage {
            protected: RetainedHistoryClassUsage {
                item_count: 1,
                charged_bytes: 2,
            },
            ordinary: RetainedHistoryClassUsage {
                item_count: 1,
                charged_bytes: 1,
            },
        }
    );
    assert!(history.entries().is_empty());
    assert_eq!(history.generation(), 1);
    assert_eq!(old_snapshot.entries().len(), 2);
}

#[test]
fn reset_rechecks_class_budgets_and_returns_bounded_omission_summary() {
    let clones = Arc::new(AtomicUsize::new(0));
    let mut history = RetainedHistory::new(budget(class_budget(0, 100), class_budget(2, 5)));
    let old = CountedValue::new("old", &clones);
    let a = CountedValue::new("a", &clones);
    let b = CountedValue::new("b", &clones);
    let c = CountedValue::new("c", &clones);
    let oversized = CountedValue::new("oversized", &clones);
    let disabled = CountedValue::new("disabled", &clones);
    history.append(RetainedHistoryClass::Ordinary, 1, &old);
    let old_snapshot = history.snapshot();

    let outcome = history.reset([
        (RetainedHistoryClass::Ordinary, 2, &a),
        (RetainedHistoryClass::Ordinary, 2, &b),
        (RetainedHistoryClass::Ordinary, 2, &c),
        (RetainedHistoryClass::Ordinary, 6, &oversized),
        (RetainedHistoryClass::Protected, 1, &disabled),
    ]);

    assert_eq!(outcome.cleared.ordinary.item_count, 1);
    assert_eq!(outcome.admitted_items, 3);
    assert_eq!(outcome.rejected.class_disabled, 1);
    assert_eq!(outcome.rejected.entry_exceeds_byte_limit, 1);
    assert_eq!(outcome.rejected.accounting_overflow, 0);
    assert_eq!(outcome.evicted.ordinary.item_count, 1);
    assert_eq!(outcome.evicted.ordinary.charged_bytes, 2);
    assert!(!outcome.summary_counts_saturated);
    assert_eq!(
        history
            .entries()
            .iter()
            .map(|entry| entry.payload().label)
            .collect::<Vec<_>>(),
        vec!["b", "c"]
    );
    assert_eq!(history.accounting().ordinary.item_count, 2);
    assert_eq!(history.accounting().ordinary.charged_bytes, 4);
    assert_eq!(old_snapshot.entries()[0].payload().label, "old");
    assert_eq!(clones.load(Ordering::SeqCst), 4);
}

#[test]
fn reset_aggregate_saturates_instead_of_wrapping_charged_bytes() {
    let mut history = RetainedHistory::new(budget(class_budget(0, 0), class_budget(1, usize::MAX)));
    let one = String::from("one");
    let two = String::from("two");
    let three = String::from("three");

    let outcome = history.reset([
        (RetainedHistoryClass::Ordinary, usize::MAX, &one),
        (RetainedHistoryClass::Ordinary, usize::MAX, &two),
        (RetainedHistoryClass::Ordinary, usize::MAX, &three),
    ]);

    assert_eq!(outcome.admitted_items, 3);
    assert_eq!(outcome.evicted.ordinary.item_count, 2);
    assert_eq!(outcome.evicted.ordinary.charged_bytes, usize::MAX);
    assert!(outcome.summary_counts_saturated);
    assert_eq!(history.entries().len(), 1);
    assert_eq!(history.accounting().ordinary.item_count, 1);
    assert_eq!(history.accounting().ordinary.charged_bytes, usize::MAX);
}
