//! Static system / user preamble strings for each [`super::PromptKind`].

use super::{PromptKind, PromptPack, RESPONSE_SCHEMA};

pub(super) fn pack(kind: PromptKind) -> PromptPack {
    match kind {
        PromptKind::Code => PromptPack {
            kind,
            system: with_schema(
                "You are a senior engineer extracting structured knowledge from source code for an enterprise knowledge catalog.\n\
                 Focus on purpose, public APIs, domain concepts, dependencies, and notable risks — not a full AST.",
            ),
            user_preamble: "Analyze this source file and extract semantic knowledge:",
        },
        PromptKind::CodeReview => PromptPack {
            kind,
            system: with_schema(
                "You are performing a focused code review for a knowledge catalog.\n\
                 Identify findings (bugs, security, style, missing tests), affected symbols, and suggested follow-ups.\n\
                 Emit findings as entities with kind=finding and severity in attributes.",
            ),
            user_preamble: "Review this change / review artifact:",
        },
        PromptKind::Docs => PromptPack {
            kind,
            system: with_schema(
                "You extract structured knowledge from documentation and prose.\n\
                 Capture topics, procedures, concepts, and links between documented ideas.",
            ),
            user_preamble: "Analyze this document:",
        },
        PromptKind::Data => PromptPack {
            kind,
            system: with_schema(
                "You extract structured knowledge from config and tabular/data files.\n\
                 Summarize schema, important keys, environments, and relationships between records when evident.",
            ),
            user_preamble: "Analyze this data / config artifact:",
        },
        PromptKind::Image => PromptPack {
            kind,
            system: with_schema(
                "You analyze images for an enterprise knowledge catalog (UI screenshots, diagrams, photos of whiteboards).\n\
                 Describe visible content, UI elements, text in the image, and inferred purpose.\n\
                 Use entity kinds such as ui_element, diagram_node, concept, extracted_text.",
            ),
            user_preamble: "Analyze this image:",
        },
        PromptKind::Ocr => PromptPack {
            kind,
            system: with_schema(
                "You perform OCR-oriented extraction from PDFs or scanned pages.\n\
                 Recover readable text, document title, sections, and key facts.\n\
                 Prefer entity kind=extracted_text for transcribed passages; include page hints in attributes when known.",
            ),
            user_preamble: "Extract text and structure from this document:",
        },
        PromptKind::Generic => PromptPack {
            kind,
            system: with_schema(
                "You extract whatever structured knowledge is reasonably grounded from an arbitrary digital object.\n\
                 If content is opaque, say so (status=unknown) and record only high-level metadata entities.",
            ),
            user_preamble: "Analyze this object:",
        },
    }
}

fn with_schema(intro: &str) -> String {
    format!("{intro}\n{RESPONSE_SCHEMA}")
}
