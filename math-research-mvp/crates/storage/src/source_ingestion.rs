use research_domain::SourceDraft;

pub(crate) const LEAD_UNVERIFIED: &str = "lead_unverified";
pub(crate) const REPORTED_UNVERIFIED: &str = "reported_unverified";
pub(crate) const NOT_APPLICABLE: &str = "not_applicable";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceAdmission {
    Lead,
    Reported,
    NotApplicable,
}

impl SourceAdmission {
    pub(crate) const fn stored_status(self) -> &'static str {
        match self {
            Self::Lead => LEAD_UNVERIFIED,
            Self::Reported => REPORTED_UNVERIFIED,
            Self::NotApplicable => NOT_APPLICABLE,
        }
    }

    pub(crate) const fn event_type(self) -> &'static str {
        match self {
            Self::Lead => "source.lead.recorded",
            Self::Reported => "source.reported",
            Self::NotApplicable => "source.not_applicable.recorded",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceAssessment {
    pub(crate) admission: SourceAdmission,
    pub(crate) rejection_reasons: Vec<&'static str>,
}

/// Classify an untrusted source draft after the parent process has checked any
/// claimed full-text artifact. A lead is deliberately useful for discovery but
/// deliberately insufficient as mathematical evidence.
pub(crate) fn assess_source_draft(
    source: &SourceDraft,
    is_literature_researcher: bool,
    has_stable_identifier: bool,
    fulltext_artifact_valid: bool,
) -> SourceAssessment {
    let admission = match source.status.trim() {
        LEAD_UNVERIFIED => SourceAdmission::Lead,
        "reported" | "possibly_applicable" | REPORTED_UNVERIFIED => SourceAdmission::Reported,
        NOT_APPLICABLE => SourceAdmission::NotApplicable,
        _ => {
            return SourceAssessment {
                admission: SourceAdmission::Lead,
                rejection_reasons: vec!["invalid_source_status"],
            };
        }
    };
    let mut rejection_reasons = Vec::new();
    if source.title.trim().is_empty() {
        rejection_reasons.push("missing_title");
    }
    if !has_stable_identifier {
        rejection_reasons.push("missing_stable_identifier");
    }
    match admission {
        SourceAdmission::Lead => {
            if !is_literature_researcher {
                rejection_reasons.push("lead_status_requires_literature_researcher");
            }
        }
        SourceAdmission::Reported => {
            if source
                .theorem_reference
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                rejection_reasons.push("missing_theorem_or_content_locator");
            }
            if source
                .statement_excerpt
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            {
                rejection_reasons.push("missing_statement_excerpt");
            }
            if !fulltext_artifact_valid {
                rejection_reasons.push("missing_or_invalid_fulltext_artifact");
            }
        }
        SourceAdmission::NotApplicable => {
            if source.applicability.trim().is_empty() {
                rejection_reasons.push("missing_not_applicable_reason");
            }
        }
    }
    SourceAssessment {
        admission,
        rejection_reasons,
    }
}

pub(crate) fn status_can_support_candidate(status: &str) -> bool {
    matches!(status, REPORTED_UNVERIFIED | "admitted")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(status: &str) -> SourceDraft {
        SourceDraft {
            title: "Paper".into(),
            authors: vec![],
            url: Some("https://example.test/paper".into()),
            citation_key: None,
            theorem_reference: None,
            statement_excerpt: None,
            assumptions: vec![],
            applicability: "needs inspection".into(),
            status: status.into(),
            retrieval_query: None,
            document_version: None,
            fulltext_path: None,
            fulltext_sha256: None,
            fulltext_artifact_id: None,
        }
    }

    #[test]
    fn lead_needs_only_identity_when_reported_by_literature_worker() {
        let assessment = assess_source_draft(&draft(LEAD_UNVERIFIED), true, true, false);
        assert_eq!(assessment.admission, SourceAdmission::Lead);
        assert!(assessment.rejection_reasons.is_empty());
    }

    #[test]
    fn reported_source_still_needs_locator_statement_and_verified_fulltext() {
        let assessment = assess_source_draft(&draft("reported"), true, true, false);
        assert_eq!(assessment.admission, SourceAdmission::Reported);
        assert_eq!(
            assessment.rejection_reasons,
            vec![
                "missing_theorem_or_content_locator",
                "missing_statement_excerpt",
                "missing_or_invalid_fulltext_artifact"
            ]
        );
    }

    #[test]
    fn not_applicable_needs_reason_but_not_fulltext_or_locator() {
        let assessment = assess_source_draft(&draft(NOT_APPLICABLE), true, true, false);
        assert_eq!(assessment.admission, SourceAdmission::NotApplicable);
        assert!(assessment.rejection_reasons.is_empty());
    }

    #[test]
    fn unknown_status_and_lead_from_non_literature_worker_fail_closed() {
        assert_eq!(
            assess_source_draft(&draft("maybe"), true, true, true).rejection_reasons,
            vec!["invalid_source_status"]
        );
        assert_eq!(
            assess_source_draft(&draft(LEAD_UNVERIFIED), false, true, false).rejection_reasons,
            vec!["lead_status_requires_literature_researcher"]
        );
    }
}
