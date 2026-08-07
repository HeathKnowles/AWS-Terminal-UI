use std::collections::HashMap;

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Matcher, Utf32String};

use crate::loader::{OutputField, ServiceTree};

#[derive(Debug, Clone)]
pub enum SearchKind {
    Service,
    Operation,
}

#[derive(Debug, Clone)]
pub struct SearchRow {
    pub kind: SearchKind,
    /// Service cli name, e.g. "ec2"
    pub service: String,
    /// Operation cli name, or "" for service rows
    pub operation: String,
    /// Full display text, e.g. "ec2 describe-subnets"
    pub haystack: Utf32String,
    /// Fields reachable from this operation's output (for "grep" queries)
    pub output_fields: Vec<OutputField>,
}

pub struct SearchIndex {
    pub rows: Vec<SearchRow>,
    pub by_service: HashMap<String, usize>,
    pub by_service_op: HashMap<(String, String), usize>,
}

impl SearchIndex {
    pub fn build(tree: &ServiceTree) -> Self {
        let mut rows = Vec::new();
        let mut by_service = HashMap::new();
        let mut by_service_op = HashMap::new();
        for svc in &tree.services {
            rows.push(SearchRow {
                kind: SearchKind::Service,
                service: svc.cli_name.clone(),
                operation: String::new(),
                haystack: Utf32String::from(format!("{} {}", svc.cli_name, svc.full_name)),
                output_fields: Vec::new(),
            });
            by_service.insert(svc.cli_name.clone(), rows.len() - 1);
            for op in &svc.ops {
                rows.push(SearchRow {
                    kind: SearchKind::Operation,
                    service: svc.cli_name.clone(),
                    operation: op.cli_name.clone(),
                    haystack: Utf32String::from(format!(
                        "{} {}",
                        svc.cli_name, op.cli_name
                    )),
                    output_fields: op.output_fields.clone(),
                });
                by_service_op.insert(
                    (svc.cli_name.clone(), op.cli_name.clone()),
                    rows.len() - 1,
                );
            }
        }
        SearchIndex {
            rows,
            by_service,
            by_service_op,
        }
    }

    /// Return (row_index, score) for rows matching `query`, best first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(usize, u32)> {
        let mut matcher = Matcher::new(nucleo::Config::DEFAULT);
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let mut results: Vec<(usize, u32)> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| {
                pattern
                    .score(row.haystack.slice(..), &mut matcher)
                    .map(|score| (i, score))
            })
            .collect();
        results.sort_by(|a, b| b.1.cmp(&a.1));
        results.truncate(limit);
        results
    }

    /// Exact lookup helpers for the builder/runner.
    pub fn get_service(&self, service: &str) -> Option<&SearchRow> {
        self.by_service.get(service).map(|&i| &self.rows[i])
    }

    pub fn get_operation(&self, service: &str, operation: &str) -> Option<&SearchRow> {
        self.by_service_op
            .get(&(service.to_string(), operation.to_string()))
            .map(|&i| &self.rows[i])
    }
}
