//! The code languages pragma checks in v1, by file extension.

use ast_grep_language::SupportLang;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CodeLang {
    TypeScript,
    Tsx,
    JavaScript,
}

impl CodeLang {
    pub fn for_path(rel_path: &str) -> Option<CodeLang> {
        let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
        match name.rsplit_once('.')?.1 {
            "ts" | "mts" | "cts" => Some(CodeLang::TypeScript),
            "tsx" => Some(CodeLang::Tsx),
            "js" | "mjs" | "cjs" | "jsx" => Some(CodeLang::JavaScript),
            _ => None,
        }
    }

    pub fn support(self) -> SupportLang {
        match self {
            CodeLang::TypeScript => SupportLang::TypeScript,
            CodeLang::Tsx => SupportLang::Tsx,
            CodeLang::JavaScript => SupportLang::JavaScript,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_language_from_the_extension() {
        assert_eq!(CodeLang::for_path("src/a.ts"), Some(CodeLang::TypeScript));
        assert_eq!(
            CodeLang::for_path("src/types.d.ts"),
            Some(CodeLang::TypeScript)
        );
        assert_eq!(CodeLang::for_path("src/view.tsx"), Some(CodeLang::Tsx));
        assert_eq!(CodeLang::for_path("tool/x.mjs"), Some(CodeLang::JavaScript));
        assert_eq!(CodeLang::for_path("docs.v2/readme"), None);
        assert_eq!(CodeLang::for_path("README.md"), None);
    }
}
