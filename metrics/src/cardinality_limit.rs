//! Prometheus label cardinality limiter.
//!
//! High-cardinality labels (e.g. per-route, per-name) can cause unbounded
//! memory growth in Prometheus.  This module provides [`LabelCardinalityLimiter`],
//! which caps the number of distinct label values tracked per metric and
//! redirects overflows into a single `"_other"` bucket.
//!
//! # How it works
//!
//! For each metric name (e.g. `"router_core_route_paused"`) the limiter
//! maintains an ordered set of distinct label-value combinations seen so far.
//! When a new label set arrives:
//!
//! * If fewer than `max_cardinality` distinct values exist → the label is
//!   **accepted** and tracked normally.
//! * If the cap has been reached → the label is **mapped** to `"_other"`.
//!   All overflow values are accumulated in a single Prometheus time-series
//!   so that the total is still observable.
//!
//! The limiter is per-metric: `"router_core_route_paused"` and
//! `"router_middleware_circuit_open"` each get their own budget.
//!
//! # Configuration
//!
//! The default cap is **100** distinct label values per metric.  It is
//! configurable via:
//!
//! * CLI flag: `--max-cardinality <N>`
//! * Environment variable: `ROUTER_MAX_CARDINALITY=<N>`
//!
//! # Metrics affected
//!
//! Only metrics that carry a user-controlled label (route, name) are
//! subject to limiting.  The `contract` label is excluded because contract
//! IDs are deployment-controlled and bounded.

use dashmap::DashMap;
use std::collections::HashSet;
use std::sync::Arc;

/// Default maximum number of distinct label-value combinations per metric.
pub const DEFAULT_MAX_CARDINALITY: usize = 100;

/// Label value used when the cardinality cap is exceeded.
pub const OTHER_BUCKET: &str = "_other";

/// Tracks and limits the number of distinct label values per metric.
///
/// Thread-safe — uses `DashMap` for concurrent access from the async scrape loop.
#[derive(Clone)]
pub struct LabelCardinalityLimiter {
    /// Per-metric set of accepted label-value combos.
    /// Key = metric name, Value = set of accepted label strings.
    seen: Arc<DashMap<String, HashSet<String>>>,
    /// Maximum distinct label values allowed per metric.
    max_cardinality: usize,
}

impl LabelCardinalityLimiter {
    /// Create a new limiter with the given per-metric cap.
    pub fn new(max_cardinality: usize) -> Self {
        Self {
            seen: Arc::new(DashMap::new()),
            max_cardinality,
        }
    }

    /// Return the configured max cardinality.
    pub fn max_cardinality(&self) -> usize {
        self.max_cardinality
    }

    /// Map a high-cardinality label value through the limiter.
    ///
    /// Returns the original `value` if it is within budget, or [`OTHER_BUCKET`]
    /// if the cap has been exceeded.
    pub fn map_label(&self, metric_name: &str, value: &str) -> String {
        if self.max_cardinality == 0 {
            return OTHER_BUCKET.to_string();
        }

        let mut entry = self.seen.entry(metric_name.to_string()).or_default();

        // Already accepted — pass through.
        if entry.contains(value) {
            return value.to_string();
        }

        // Under the cap — accept and track.
        if entry.len() < self.max_cardinality {
            entry.insert(value.to_string());
            return value.to_string();
        }

        // Cap reached — redirect to "_other".
        OTHER_BUCKET.to_string()
    }

    /// Map a composite label (e.g. route+name) through the limiter.
    ///
    /// The composite key is `"{parts.join('/')}"`.  The first part (typically
    /// the contract ID) is excluded from the cardinality check; only the
    /// user-controlled tail is considered.
    pub fn map_composite_label(&self, metric_name: &str, parts: &[&str]) -> Vec<String> {
        // Contract IDs are deployment-controlled; only the tail is user-controlled.
        // We join the full label set as a key but track cardinality on the tail.
        let label_key = parts.join("/");
        let tail = parts.last().copied().unwrap_or("_unknown");

        // Check tail cardinality specifically.
        if self.max_cardinality == 0 {
            return parts
                .iter()
                .enumerate()
                .map(|(i, _)| {
                    if i == parts.len() - 1 {
                        OTHER_BUCKET.to_string()
                    } else {
                        parts[i].to_string()
                    }
                })
                .collect();
        }

        let metric_key = format!("{metric_name}:tail");
        let mut entry = self.seen.entry(metric_key).or_default();

        if entry.contains(tail) {
            return parts.iter().map(|s| s.to_string()).collect();
        }

        if entry.len() < self.max_cardinality {
            entry.insert(tail.to_string());
            return parts.iter().map(|s| s.to_string()).collect();
        }

        // Replace only the tail with OTHER_BUCKET.
        parts
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                if i == parts.len() - 1 {
                    OTHER_BUCKET.to_string()
                } else {
                    s.to_string()
                }
            })
            .collect()
    }

    /// Return the number of distinct label values currently tracked for a metric.
    pub fn tracked_count(&self, metric_name: &str) -> usize {
        self.seen
            .get(metric_name)
            .map(|s| s.len())
            .unwrap_or(0)
    }

    /// Reset tracked labels for a metric (useful for tests).
    pub fn reset(&self, metric_name: &str) {
        self.seen.remove(metric_name);
    }

    /// Reset all tracked labels (useful for tests).
    pub fn reset_all(&self) {
        self.seen.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_under_cap_passes_through() {
        let limiter = LabelCardinalityLimiter::new(5);
        let result = limiter.map_label("test_metric", "route_a");
        assert_eq!(result, "route_a");
    }

    #[test]
    fn test_same_label_repeated_is_free() {
        let limiter = LabelCardinalityLimiter::new(3);
        for _ in 0..100 {
            let result = limiter.map_label("test_metric", "route_a");
            assert_eq!(result, "route_a");
        }
    }

    #[test]
    fn test_at_cap_redirects_to_other() {
        let limiter = LabelCardinalityLimiter::new(2);
        assert_eq!(limiter.map_label("m", "a"), "a");
        assert_eq!(limiter.map_label("m", "b"), "b");
        // Third distinct value → overflow
        assert_eq!(limiter.map_label("m", "c"), OTHER_BUCKET);
        assert_eq!(limiter.map_label("m", "d"), OTHER_BUCKET);
    }

    #[test]
    fn test_zero_cap_everything_goes_to_other() {
        let limiter = LabelCardinalityLimiter::new(0);
        assert_eq!(limiter.map_label("m", "a"), OTHER_BUCKET);
        assert_eq!(limiter.map_label("m", "b"), OTHER_BUCKET);
    }

    #[test]
    fn test_independent_metrics() {
        let limiter = LabelCardinalityLimiter::new(2);
        assert_eq!(limiter.map_label("m1", "a"), "a");
        assert_eq!(limiter.map_label("m2", "b"), "b");
        // m1 still has room
        assert_eq!(limiter.map_label("m1", "c"), "c");
        // m2 still has room
        assert_eq!(limiter.map_label("m2", "d"), "d");
        // Now both are full
        assert_eq!(limiter.map_label("m1", "e"), OTHER_BUCKET);
        assert_eq!(limiter.map_label("m2", "f"), OTHER_BUCKET);
    }

    #[test]
    fn test_tracked_count() {
        let limiter = LabelCardinalityLimiter::new(10);
        assert_eq!(limiter.tracked_count("m"), 0);
        limiter.map_label("m", "a");
        limiter.map_label("m", "b");
        assert_eq!(limiter.tracked_count("m"), 2);
    }

    #[test]
    fn test_reset() {
        let limiter = LabelCardinalityLimiter::new(1);
        limiter.map_label("m", "a");
        assert_eq!(limiter.tracked_count("m"), 1);
        limiter.reset("m");
        assert_eq!(limiter.tracked_count("m"), 0);
        // Now we can add again
        assert_eq!(limiter.map_label("m", "b"), "b");
    }

    #[test]
    fn test_reset_all() {
        let limiter = LabelCardinalityLimiter::new(10);
        limiter.map_label("m1", "a");
        limiter.map_label("m2", "b");
        limiter.reset_all();
        assert_eq!(limiter.tracked_count("m1"), 0);
        assert_eq!(limiter.tracked_count("m2"), 0);
    }

    #[test]
    fn test_composite_label_under_cap() {
        let limiter = LabelCardinalityLimiter::new(5);
        let result = limiter.map_composite_label("test", &["contract_id", "oracle"]);
        assert_eq!(result, vec!["contract_id", "oracle"]);
    }

    #[test]
    fn test_composite_label_overflow_replaces_tail() {
        let limiter = LabelCardinalityLimiter::new(2);
        limiter.map_composite_label("m", &["cid", "route1"]);
        limiter.map_composite_label("m", &["cid", "route2"]);
        let result = limiter.map_composite_label("m", &["cid", "route3"]);
        assert_eq!(result, vec!["cid", OTHER_BUCKET]);
    }

    #[test]
    fn test_composite_label_overflow_reuses_other() {
        let limiter = LabelCardinalityLimiter::new(1);
        limiter.map_composite_label("m", &["cid", "route1"]);
        let r1 = limiter.map_composite_label("m", &["cid", "route2"]);
        let r2 = limiter.map_composite_label("m", &["cid", "route3"]);
        assert_eq!(r1, vec!["cid", OTHER_BUCKET]);
        assert_eq!(r2, vec!["cid", OTHER_BUCKET]);
    }

    #[test]
    fn test_max_cardinality_accessor() {
        let limiter = LabelCardinalityLimiter::new(42);
        assert_eq!(limiter.max_cardinality(), 42);
    }

    #[test]
    fn test_clone_shares_state() {
        let limiter = LabelCardinalityLimiter::new(2);
        let limiter2 = limiter.clone();
        limiter.map_label("m", "a");
        assert_eq!(limiter2.tracked_count("m"), 1);
    }
}
