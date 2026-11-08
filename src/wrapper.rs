//! Wraps the user's script so the tool can tell that it started and how it ended.
//!
//! Studio's exit code says nothing about the script, and its output file echoes
//! the script source before running it. Markers are therefore assembled at runtime
//! (`"a".."b"`), so the echo can never contain the finished marker text. The
//! wrapper adds no leading lines, which keeps line numbers in error messages correct.

const BOM: &[u8] = b"\xEF\xBB\xBF";

pub struct Markers {
    prefix: String,
    pub begin: String,
    pub ok: String,
    pub fail: String,
    pub end: String,
    /// How the first line of the script looks in Studio's echo of the source.
    pub source_echo: String,
}

impl Markers {
    pub fn new(nonce: &str) -> Self {
        let prefix = format!("__STUDIO_RUN_{nonce}_");
        Markers {
            begin: format!("{prefix}BEGIN__"),
            ok: format!("{prefix}OK__"),
            fail: format!("{prefix}FAIL__"),
            end: format!("{prefix}END__"),
            source_echo: format!("> print(\"{prefix}\""),
            prefix,
        }
    }

    /// Returns the wrapped script. The source is kept as bytes so its encoding
    /// passes through untouched; any leading UTF-8 BOMs are removed.
    pub fn wrap(&self, mut source: &[u8]) -> Vec<u8> {
        while let Some(rest) = source.strip_prefix(BOM) {
            source = rest;
        }
        let head = format!(
            "print({});local __studioRunOk,__studioRunErr=xpcall(function(...) ",
            self.split("BEGIN__")
        );
        // The function body keeps a top-level `return` valid, and the newline
        // stops a comment on the user's last line from swallowing the suffix.
        let tail = format!(
            "\nend,function(e) return debug.traceback(tostring(e),2) end);\
             if __studioRunOk then print({}) \
             else print({});print(__studioRunErr);print({}) end\n",
            self.split("OK__"),
            self.split("FAIL__"),
            self.split("END__")
        );
        let mut wrapped = Vec::with_capacity(head.len() + source.len() + tail.len());
        wrapped.extend_from_slice(head.as_bytes());
        wrapped.extend_from_slice(source);
        wrapped.extend_from_slice(tail.as_bytes());
        wrapped
    }

    fn split(&self, suffix: &str) -> String {
        format!("\"{}\"..\"{suffix}\"", self.prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_script_matches_reference_format() {
        let markers = Markers::new("abc");
        let wrapped = String::from_utf8(markers.wrap(b"print(1)")).unwrap();
        assert_eq!(
            wrapped,
            "print(\"__STUDIO_RUN_abc_\"..\"BEGIN__\");local __studioRunOk,__studioRunErr=xpcall(function(...) print(1)\n\
             end,function(e) return debug.traceback(tostring(e),2) end);\
             if __studioRunOk then print(\"__STUDIO_RUN_abc_\"..\"OK__\") \
             else print(\"__STUDIO_RUN_abc_\"..\"FAIL__\");print(__studioRunErr);print(\"__STUDIO_RUN_abc_\"..\"END__\") end\n"
        );
    }

    #[test]
    fn finished_markers_never_appear_in_wrapped_source() {
        let markers = Markers::new("abc");
        let wrapped = String::from_utf8(markers.wrap(b"print(1)")).unwrap();
        for marker in [&markers.begin, &markers.ok, &markers.fail, &markers.end] {
            assert!(!wrapped.contains(marker.as_str()), "{marker}");
        }
        assert!(format!("> {wrapped}").starts_with(&markers.source_echo));
    }

    #[test]
    fn strips_leading_boms() {
        let markers = Markers::new("n");
        let wrapped = markers.wrap(b"\xEF\xBB\xBF\xEF\xBB\xBFprint(1)");
        assert!(!wrapped.windows(3).any(|w| w == BOM));
    }
}
