//! Line ends, which the editor keeps the way the document came.
//!
//! Every edit splices lines back together with LF, and a browser's textarea
//! hands back LF whatever it was given. A document written with CRLF is
//! therefore edited as LF and given its CRLF back on the way out; otherwise
//! changing one word in it would change every line.

/// Whether `source` is written with CRLF line ends.
///
/// One is enough to say so: a document mixing the two comes back with CRLF
/// throughout, which is the nearer of the two to what it was.
pub fn is_crlf(source: &str) -> bool {
    source.contains("\r\n")
}

/// `source` with LF line ends.
pub fn to_lf(source: String) -> String {
    if is_crlf(&source) {
        source.replace("\r\n", "\n")
    } else {
        source
    }
}

/// `source`, edited with LF, given back the line ends it came with.
pub fn restore(source: String, crlf: bool) -> String {
    if crlf {
        to_lf(source).replace('\n', "\r\n")
    } else {
        source
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lf_documents_pass_through_untouched() {
        let doc = "= Title\n\nWords.\n";

        assert!(!is_crlf(doc));
        assert_eq!(to_lf(doc.to_string()), doc);
        assert_eq!(restore(doc.to_string(), false), doc);
    }

    #[test]
    fn crlf_documents_come_back_as_they_went_in() {
        let doc = "= Title\r\n:keywords: a\r\n\r\nWords.\r\n";

        assert!(is_crlf(doc));
        let edited = to_lf(doc.to_string());
        assert_eq!(edited, "= Title\n:keywords: a\n\nWords.\n");
        assert_eq!(restore(edited, true), doc);
    }

    #[test]
    fn an_edit_to_a_crlf_document_changes_only_its_own_line() {
        let doc = "= Title\r\n\r\nOne.\r\n\r\nTwo.\r\n";
        let edited = to_lf(doc.to_string()).replace("Two.", "Three.");

        assert_eq!(
            restore(edited, true),
            "= Title\r\n\r\nOne.\r\n\r\nThree.\r\n"
        );
    }

    #[test]
    fn a_lone_carriage_return_is_not_a_line_end() {
        assert!(!is_crlf("a\rb\n"));
        assert_eq!(restore("a\rb\n".to_string(), false), "a\rb\n");
    }
}
