//! Bounded, provider-neutral retention for original annotated transcript evidence.
//!
//! The charged-byte limits are caller-supplied accounting units, not allocator or process-wide
//! memory quotas. They apply to the current live history only: snapshots can keep evicted payloads
//! alive after they leave the live history. The caller chooses each item's protected/ordinary
//! class and its charge; this module does not transform payloads or infer approval semantics.
//! Use this boundary for original annotated transcript evidence, not derived approval scores or
//! trusted `ExtensionData` answers.
//!
//! Append/reset outcomes report rejected or evicted evidence. Callers must not infer completeness
//! after either kind of omission. A generation identifies a retained prefix within this container,
//! not across sessions; downstream users must pair it with the relevant lifecycle identity.

use std::sync::Arc;

/// Caller-supplied retention classification. The classes have independent budgets and eviction.
///
/// `Protected` indicates eviction priority only; it does not imply trust, approval, permission, or
/// authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedHistoryClass {
    /// Evidence that is evicted only as needed to satisfy its own class budget.
    Protected,
    /// Evidence whose overflow also removes at least half of existing entries first.
    Ordinary,
}

/// Independent count and charged-byte limits for one retention class.
///
/// A zero value for either limit disables that class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainedHistoryClassBudget {
    pub max_items: usize,
    pub max_charged_bytes: usize,
}

impl RetainedHistoryClassBudget {
    pub const fn new(max_items: usize, max_charged_bytes: usize) -> Self {
        Self {
            max_items,
            max_charged_bytes,
        }
    }

    fn is_disabled(self) -> bool {
        self.max_items == 0 || self.max_charged_bytes == 0
    }
}

/// Both independent class budgets for one live retained history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainedHistoryBudget {
    pub protected: RetainedHistoryClassBudget,
    pub ordinary: RetainedHistoryClassBudget,
}

impl RetainedHistoryBudget {
    pub const fn new(
        protected: RetainedHistoryClassBudget,
        ordinary: RetainedHistoryClassBudget,
    ) -> Self {
        Self {
            protected,
            ordinary,
        }
    }

    fn for_class(self, class: RetainedHistoryClass) -> RetainedHistoryClassBudget {
        match class {
            RetainedHistoryClass::Protected => self.protected,
            RetainedHistoryClass::Ordinary => self.ordinary,
        }
    }
}

/// Current item-count and charged-byte usage for one class.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetainedHistoryClassUsage {
    pub item_count: usize,
    pub charged_bytes: usize,
}

/// Current item-count and charged-byte usage for both classes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetainedHistoryUsage {
    pub protected: RetainedHistoryClassUsage,
    pub ordinary: RetainedHistoryClassUsage,
}

impl RetainedHistoryUsage {
    pub fn for_class(self, class: RetainedHistoryClass) -> RetainedHistoryClassUsage {
        match class {
            RetainedHistoryClass::Protected => self.protected,
            RetainedHistoryClass::Ordinary => self.ordinary,
        }
    }

    fn for_class_mut(&mut self, class: RetainedHistoryClass) -> &mut RetainedHistoryClassUsage {
        match class {
            RetainedHistoryClass::Protected => &mut self.protected,
            RetainedHistoryClass::Ordinary => &mut self.ordinary,
        }
    }

    fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            protected: checked_sub_class_usage(self.protected, other.protected)?,
            ordinary: checked_sub_class_usage(self.ordinary, other.ordinary)?,
        })
    }
}

fn checked_sub_class_usage(
    left: RetainedHistoryClassUsage,
    right: RetainedHistoryClassUsage,
) -> Option<RetainedHistoryClassUsage> {
    Some(RetainedHistoryClassUsage {
        item_count: left.item_count.checked_sub(right.item_count)?,
        charged_bytes: left.charged_bytes.checked_sub(right.charged_bytes)?,
    })
}

/// One retained payload with its caller-supplied class and charged-byte measurement.
#[derive(Debug)]
pub struct RetainedHistoryEntry<T> {
    class: RetainedHistoryClass,
    charged_bytes: usize,
    payload: Arc<T>,
}

impl<T> Clone for RetainedHistoryEntry<T> {
    fn clone(&self) -> Self {
        Self {
            class: self.class,
            charged_bytes: self.charged_bytes,
            payload: Arc::clone(&self.payload),
        }
    }
}

impl<T> RetainedHistoryEntry<T> {
    pub fn class(&self) -> RetainedHistoryClass {
        self.class
    }

    pub fn charged_bytes(&self) -> usize {
        self.charged_bytes
    }

    pub fn payload(&self) -> &T {
        self.payload.as_ref()
    }
}

/// Append result. An admitted item can evict earlier items from its own class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedHistoryAppendOutcome {
    Appended { evicted: RetainedHistoryClassUsage },
    Rejected(RetainedHistoryRejection),
}

/// Why an append or reset input was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetainedHistoryRejection {
    ClassDisabled {
        class: RetainedHistoryClass,
    },
    EntryExceedsByteLimit {
        class: RetainedHistoryClass,
        charged_bytes: usize,
        max_charged_bytes: usize,
    },
    /// Internal checked accounting could not be represented without wrapping.
    AccountingOverflow,
}

/// Saturating counts of rejected inputs encountered while rebuilding a history.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetainedHistoryResetRejectionCounts {
    pub class_disabled: usize,
    pub entry_exceeds_byte_limit: usize,
    pub accounting_overflow: usize,
}

/// Bounded aggregate report for a reset-and-rebuild operation.
///
/// `admitted_items` counts inputs accepted at some point during rebuilding, including entries that
/// a later input may evict. `evicted` counts those rebuild-time omissions; `cleared` describes the
/// prior live history removed at reset. Aggregate counters saturate rather than wrapping and set
/// `summary_counts_saturated` if their exact total exceeded `usize`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetainedHistoryResetOutcome {
    pub cleared: RetainedHistoryUsage,
    pub admitted_items: usize,
    pub rejected: RetainedHistoryResetRejectionCounts,
    pub evicted: RetainedHistoryUsage,
    pub summary_counts_saturated: bool,
}

/// Result of fail-closed truncation. The matching boundary itself is removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetainedHistoryTruncateOutcome {
    pub boundary_found: bool,
    pub removed: RetainedHistoryUsage,
}

/// Cheap immutable view of one live-history prefix.
///
/// The retained membership, order, accounting, and generation do not change when the live history
/// is later appended, reset, or truncated. Payloads are shared; deep immutability of `T`, including
/// any interior mutability it contains, remains the caller's responsibility.
#[derive(Debug)]
pub struct RetainedHistorySnapshot<T> {
    entries: Arc<Vec<RetainedHistoryEntry<T>>>,
    generation: u64,
    usage: RetainedHistoryUsage,
}

impl<T> Clone for RetainedHistorySnapshot<T> {
    fn clone(&self) -> Self {
        Self {
            entries: Arc::clone(&self.entries),
            generation: self.generation,
            usage: self.usage,
        }
    }
}

impl<T> RetainedHistorySnapshot<T> {
    pub fn entries(&self) -> &[RetainedHistoryEntry<T>] {
        self.entries.as_slice()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn accounting(&self) -> RetainedHistoryUsage {
        self.usage
    }
}

/// Per-container bounded retention for ordered transcript evidence.
///
/// Snapshots and clones share the backing entries and payloads. Mutations use copy-on-write so a
/// prior snapshot keeps its membership, order, accounting, and generation unchanged. The budget
/// applies only to this live container, not to payloads retained by snapshots.
#[derive(Debug)]
pub struct RetainedHistory<T> {
    budget: RetainedHistoryBudget,
    entries: Arc<Vec<RetainedHistoryEntry<T>>>,
    generation: u64,
    usage: RetainedHistoryUsage,
}

impl<T> Clone for RetainedHistory<T> {
    fn clone(&self) -> Self {
        Self {
            budget: self.budget,
            entries: Arc::clone(&self.entries),
            generation: self.generation,
            usage: self.usage,
        }
    }
}

impl<T> RetainedHistory<T> {
    pub fn new(budget: RetainedHistoryBudget) -> Self {
        Self {
            budget,
            entries: Arc::new(Vec::new()),
            generation: 0,
            usage: RetainedHistoryUsage::default(),
        }
    }

    pub fn entries(&self) -> &[RetainedHistoryEntry<T>] {
        self.entries.as_slice()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn accounting(&self) -> RetainedHistoryUsage {
        self.usage
    }

    pub fn budget(&self) -> RetainedHistoryBudget {
        self.budget
    }

    /// Captures a shallow immutable view in O(1); payloads remain shared through `Arc<T>`.
    pub fn snapshot(&self) -> RetainedHistorySnapshot<T> {
        RetainedHistorySnapshot {
            entries: Arc::clone(&self.entries),
            generation: self.generation,
            usage: self.usage,
        }
    }

    /// Appends a borrowed payload if it fits its explicitly supplied class budget.
    ///
    /// Rejected payloads are not cloned. Ordinary overflow first removes at least half of the
    /// existing ordinary entries, then removes additional oldest ordinary entries as needed.
    /// Protected overflow removes only the oldest protected entries needed to fit.
    pub fn append(
        &mut self,
        class: RetainedHistoryClass,
        charged_bytes: usize,
        payload: &T,
    ) -> RetainedHistoryAppendOutcome
    where
        T: Clone,
    {
        self.append_inner(class, charged_bytes, payload, true)
    }

    /// Clears the current evidence and rebuilds through the same checks used by `append`.
    ///
    /// The returned aggregate stays bounded regardless of input length. Inputs are borrowed, so
    /// disabled, oversized, and otherwise rejected payloads are not cloned.
    pub fn reset<'a>(
        &mut self,
        items: impl IntoIterator<Item = (RetainedHistoryClass, usize, &'a T)>,
    ) -> RetainedHistoryResetOutcome
    where
        T: Clone + 'a,
    {
        let next_generation = self.next_generation();
        let mut outcome = RetainedHistoryResetOutcome {
            cleared: self.usage,
            ..RetainedHistoryResetOutcome::default()
        };

        self.entries = Arc::new(Vec::new());
        self.usage = RetainedHistoryUsage::default();
        self.generation = next_generation;

        for (class, charged_bytes, payload) in items {
            match self.append_inner(class, charged_bytes, payload, false) {
                RetainedHistoryAppendOutcome::Appended { evicted } => {
                    saturating_increment(
                        &mut outcome.admitted_items,
                        &mut outcome.summary_counts_saturated,
                    );
                    let class_usage = outcome.evicted.for_class_mut(class);
                    saturating_add(
                        &mut class_usage.item_count,
                        evicted.item_count,
                        &mut outcome.summary_counts_saturated,
                    );
                    saturating_add(
                        &mut class_usage.charged_bytes,
                        evicted.charged_bytes,
                        &mut outcome.summary_counts_saturated,
                    );
                }
                RetainedHistoryAppendOutcome::Rejected(rejection) => {
                    let count = match rejection {
                        RetainedHistoryRejection::ClassDisabled { .. } => {
                            &mut outcome.rejected.class_disabled
                        }
                        RetainedHistoryRejection::EntryExceedsByteLimit { .. } => {
                            &mut outcome.rejected.entry_exceeds_byte_limit
                        }
                        RetainedHistoryRejection::AccountingOverflow => {
                            &mut outcome.rejected.accounting_overflow
                        }
                    };
                    saturating_increment(count, &mut outcome.summary_counts_saturated);
                }
            }
        }

        outcome
    }

    /// Removes the first matching entry and everything after it.
    ///
    /// If no boundary matches, truncation fails closed by clearing the full live history. A call
    /// is a rollback boundary and advances the generation even when the history is already empty.
    pub fn truncate_before(
        &mut self,
        mut predicate: impl FnMut(&RetainedHistoryEntry<T>) -> bool,
    ) -> RetainedHistoryTruncateOutcome {
        let boundary = self.entries.iter().position(&mut predicate);
        let keep_len = boundary.unwrap_or(0);
        let removed = usage_for_entries(&self.entries[keep_len..])
            .expect("removed history usage must fit within current accounting");
        let remaining = self
            .usage
            .checked_sub(removed)
            .expect("truncated history usage must be a subset of current accounting");
        let next_generation = self.next_generation();

        Arc::make_mut(&mut self.entries).truncate(keep_len);
        self.usage = remaining;
        self.generation = next_generation;

        RetainedHistoryTruncateOutcome {
            boundary_found: boundary.is_some(),
            removed,
        }
    }

    fn append_inner(
        &mut self,
        class: RetainedHistoryClass,
        charged_bytes: usize,
        payload: &T,
        advance_generation_on_eviction: bool,
    ) -> RetainedHistoryAppendOutcome
    where
        T: Clone,
    {
        let class_budget = self.budget.for_class(class);
        if class_budget.is_disabled() {
            return RetainedHistoryAppendOutcome::Rejected(
                RetainedHistoryRejection::ClassDisabled { class },
            );
        }
        if charged_bytes > class_budget.max_charged_bytes {
            return RetainedHistoryAppendOutcome::Rejected(
                RetainedHistoryRejection::EntryExceedsByteLimit {
                    class,
                    charged_bytes,
                    max_charged_bytes: class_budget.max_charged_bytes,
                },
            );
        }

        let plan = match self.plan_append(class, charged_bytes, class_budget) {
            Ok(plan) => plan,
            Err(()) => {
                return RetainedHistoryAppendOutcome::Rejected(
                    RetainedHistoryRejection::AccountingOverflow,
                );
            }
        };
        let next_generation = if advance_generation_on_eviction && plan.evicted.item_count > 0 {
            Some(self.next_generation())
        } else {
            None
        };

        // Admission and checked accounting are complete before cloning the caller's payload.
        let payload = Arc::new(payload.clone());
        let entries = Arc::make_mut(&mut self.entries);
        if plan.evicted.item_count > 0 {
            let mut remaining_evictions = plan.evicted.item_count;
            entries.retain(|entry| {
                if entry.class == class && remaining_evictions > 0 {
                    remaining_evictions -= 1;
                    false
                } else {
                    true
                }
            });
        }
        entries.push(RetainedHistoryEntry {
            class,
            charged_bytes,
            payload,
        });
        self.usage = plan.usage_after_append;
        if let Some(generation) = next_generation {
            self.generation = generation;
        }

        RetainedHistoryAppendOutcome::Appended {
            evicted: plan.evicted,
        }
    }

    fn plan_append(
        &self,
        class: RetainedHistoryClass,
        charged_bytes: usize,
        class_budget: RetainedHistoryClassBudget,
    ) -> Result<AppendPlan, ()> {
        let mut class_usage = self.usage.for_class(class);
        if class_usage.item_count > class_budget.max_items
            || class_usage.charged_bytes > class_budget.max_charged_bytes
        {
            return Err(());
        }

        let mut evicted = RetainedHistoryClassUsage::default();
        let overflowed_before_eviction =
            append_would_exceed(class_usage, charged_bytes, class_budget);
        let minimum_evictions = match (class, overflowed_before_eviction) {
            (RetainedHistoryClass::Ordinary, true) => {
                class_usage.item_count / 2 + class_usage.item_count % 2
            }
            _ => 0,
        };

        if overflowed_before_eviction {
            for entry in self.entries.iter() {
                if entry.class != class {
                    continue;
                }
                let must_remove_for_minimum = evicted.item_count < minimum_evictions;
                if !must_remove_for_minimum
                    && !append_would_exceed(class_usage, charged_bytes, class_budget)
                {
                    break;
                }

                class_usage.item_count = class_usage.item_count.checked_sub(1).ok_or(())?;
                class_usage.charged_bytes = class_usage
                    .charged_bytes
                    .checked_sub(entry.charged_bytes)
                    .ok_or(())?;
                evicted.item_count = evicted.item_count.checked_add(1).ok_or(())?;
                evicted.charged_bytes = evicted
                    .charged_bytes
                    .checked_add(entry.charged_bytes)
                    .ok_or(())?;
            }
        }

        if append_would_exceed(class_usage, charged_bytes, class_budget) {
            return Err(());
        }
        class_usage.item_count = class_usage.item_count.checked_add(1).ok_or(())?;
        class_usage.charged_bytes = class_usage
            .charged_bytes
            .checked_add(charged_bytes)
            .ok_or(())?;
        if class_usage.item_count > class_budget.max_items
            || class_usage.charged_bytes > class_budget.max_charged_bytes
        {
            return Err(());
        }

        let mut usage_after_append = self.usage;
        *usage_after_append.for_class_mut(class) = class_usage;
        Ok(AppendPlan {
            evicted,
            usage_after_append,
        })
    }

    fn next_generation(&self) -> u64 {
        self.generation
            .checked_add(1)
            .expect("retained history generation exhausted")
    }
}

struct AppendPlan {
    evicted: RetainedHistoryClassUsage,
    usage_after_append: RetainedHistoryUsage,
}

fn append_would_exceed(
    usage: RetainedHistoryClassUsage,
    charged_bytes: usize,
    budget: RetainedHistoryClassBudget,
) -> bool {
    usage.item_count >= budget.max_items
        || charged_bytes > budget.max_charged_bytes - usage.charged_bytes
}

fn usage_for_entries<T>(entries: &[RetainedHistoryEntry<T>]) -> Option<RetainedHistoryUsage> {
    let mut usage = RetainedHistoryUsage::default();
    for entry in entries {
        let class_usage = usage.for_class_mut(entry.class);
        class_usage.item_count = class_usage.item_count.checked_add(1)?;
        class_usage.charged_bytes = class_usage.charged_bytes.checked_add(entry.charged_bytes)?;
    }
    Some(usage)
}

fn saturating_increment(value: &mut usize, saturated: &mut bool) {
    saturating_add(value, 1, saturated);
}

fn saturating_add(value: &mut usize, addition: usize, saturated: &mut bool) {
    if let Some(sum) = value.checked_add(addition) {
        *value = sum;
    } else {
        *value = usize::MAX;
        *saturated = true;
    }
}
