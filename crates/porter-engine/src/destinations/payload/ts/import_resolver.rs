use super::errors::{Result, SchemaParseError};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use swc_common::{FileName, SourceMap, Spanned, sync::Lrc};
use swc_ecma_ast::Module;
use swc_ecma_parser::{Parser, StringInput, Syntax, TsSyntax, lexer::Lexer};

/// One tsconfig `paths` entry: `alias` like `@/*`, `paths` relative to the base dir.
#[derive(Debug, Clone)]
pub struct PathAlias {
    pub alias: String,
    pub paths: Vec<String>,
}

/// Reads and parses one TypeScript file (TSX when the extension says so).
pub fn parse_ts(cm: &Lrc<SourceMap>, path: &Path) -> Result<Module> {
    let src = std::fs::read_to_string(path).map_err(|e| SchemaParseError::IoError {
        path: path.to_path_buf(),
        source: e,
    })?;
    let tsx = path.extension().is_some_and(|e| e == "tsx");
    parse_ts_str(cm, FileName::Real(path.to_path_buf()), src, tsx)
}

/// Parses TypeScript source. Recoverable syntax errors count as errors too.
pub fn parse_ts_str(cm: &Lrc<SourceMap>, name: FileName, src: String, tsx: bool) -> Result<Module> {
    let file = name.to_string();
    let fm = cm.new_source_file(Lrc::new(name), src);
    let lexer = Lexer::new(
        Syntax::Typescript(TsSyntax {
            tsx,
            decorators: true,
            ..Default::default()
        }),
        Default::default(),
        StringInput::from(&*fm),
        None,
    );
    let mut parser = Parser::new_from(lexer);
    let fail = |e: swc_ecma_parser::error::Error| SchemaParseError::ParseError {
        file: file.clone(),
        message: format!(
            "line {}: {}",
            cm.lookup_char_pos(e.span().lo).line,
            e.kind().msg()
        ),
    };
    let module = parser.parse_module().map_err(fail)?;
    if let Some(e) = parser.take_errors().into_iter().next() {
        return Err(fail(e));
    }
    Ok(module)
}
/// Resolves import specifiers (relative, `/`-rooted, tsconfig aliases) to files on disk.
pub struct ImportResolver {
    base_dir: PathBuf,
    alias_map: HashMap<String, Vec<PathBuf>>,
}

impl ImportResolver {
    /// `aliases` are tsconfig `paths` (see [`PathAlias`]); `None` means the default `@/` -> `src/`.
    pub fn new(base_dir: PathBuf, aliases: Option<&[PathAlias]>) -> Self {
        let mut alias_map = HashMap::new();
        match aliases {
            Some(list) => {
                for a in list {
                    let paths = a
                        .paths
                        .iter()
                        .map(|p| base_dir.join(p.trim_end_matches("/*").trim_end_matches('*')))
                        .collect();
                    alias_map.insert(a.alias.trim_end_matches("/*").to_string(), paths);
                }
            }
            None => {
                alias_map.insert("@/".to_string(), vec![base_dir.join("src")]);
            }
        }
        Self {
            base_dir,
            alias_map,
        }
    }

    pub fn resolve_import_path(&self, import_path: &str, from_file: &Path) -> Result<PathBuf> {
        // Handle different import types
        let resolved = if import_path.starts_with('.') {
            let parent = from_file
                .parent()
                .ok_or_else(|| SchemaParseError::UnresolvedImport {
                    import: import_path.to_string(),
                    file: from_file.display().to_string(),
                })?;
            parent.join(import_path)
        } else if let Some(rest) = import_path.strip_prefix('/') {
            self.base_dir.join(rest)
        } else {
            // tsconfig path alias; anything else is a node module, which isn't followed.
            return self.resolve_alias_import(import_path);
        };

        // Try to find the actual file with various extensions
        self.find_actual_file(resolved)
    }

    fn resolve_alias_import(&self, import_path: &str) -> Result<PathBuf> {
        // Try each alias - prioritize longer matches first
        let mut sorted_aliases: Vec<_> = self.alias_map.keys().collect();
        sorted_aliases.sort_by_key(|a| std::cmp::Reverse(a.len()));

        for alias in sorted_aliases {
            let base_paths = &self.alias_map[alias];

            // Check if this import matches the alias
            let remainder = if alias.ends_with('/') {
                // Alias ends with /, so we need exact prefix match
                if import_path.starts_with(alias) {
                    import_path.strip_prefix(alias).unwrap_or("")
                } else {
                    continue;
                }
            } else {
                // Alias doesn't end with /, so we need to match either:
                // 1. Exact match: import_path == alias
                // 2. Followed by /: import_path starts with alias + "/"
                if import_path == alias {
                    ""
                } else if import_path.starts_with(&format!("{}/", alias)) {
                    &import_path[alias.len() + 1..]
                } else {
                    continue;
                }
            };

            // Try each base path for this alias
            for base_path in base_paths {
                let full_path = if remainder.is_empty() {
                    base_path.clone()
                } else {
                    base_path.join(remainder)
                };

                // Check if this path exists (with various extensions)
                if let Ok(resolved) = self.find_actual_file(full_path.clone()) {
                    return Ok(resolved);
                }
            }
        }

        Err(SchemaParseError::UnresolvedImport {
            import: import_path.to_string(),
            file: "unknown".to_string(),
        })
    }

    fn find_actual_file(&self, base_path: PathBuf) -> Result<PathBuf> {
        // If it already has an extension and exists, return it
        if base_path.exists() {
            // Check if it's a directory
            if base_path.is_dir() {
                // Try to find index files in the directory
                let index_variations = vec![
                    base_path.join("index.ts"),
                    base_path.join("index.tsx"),
                    base_path.join("index.js"),
                    base_path.join("index.jsx"),
                ];

                for path in &index_variations {
                    if path.exists() {
                        return Ok(path.clone());
                    }
                }

                return Err(SchemaParseError::UnresolvedImport {
                    import: base_path.display().to_string(),
                    file: "filesystem".to_string(),
                });
            } else {
                return Ok(base_path);
            }
        }

        // Try different extensions and index files
        let variations = vec![
            base_path.with_extension("ts"),
            base_path.with_extension("tsx"),
            base_path.with_extension("js"),
            base_path.with_extension("jsx"),
            base_path.join("index.ts"),
            base_path.join("index.tsx"),
            base_path.join("index.js"),
            base_path.join("index.jsx"),
            base_path.join("config.ts"),
            base_path.join("config.tsx"),
        ];

        for path in &variations {
            if path.exists() {
                return Ok(path.clone());
            }
        }

        Err(SchemaParseError::UnresolvedImport {
            import: base_path.display().to_string(),
            file: "filesystem".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn alias_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dir = src.join("blocks/TestBlock");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.ts"), "export const a = 1").unwrap();
        let r = ImportResolver::new(tmp.path().to_path_buf(), None);
        let got = r
            .resolve_import_path(
                "@/blocks/TestBlock/config",
                &src.join("collections/test.ts"),
            )
            .unwrap();
        assert!(got.ends_with("src/blocks/TestBlock/config.ts"));
    }

    #[test]
    fn relative_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("collections")).unwrap();
        fs::write(src.join("collections/slug.ts"), "export const a = 1").unwrap();
        let r = ImportResolver::new(tmp.path().to_path_buf(), None);
        let got = r
            .resolve_import_path("./slug", &src.join("collections/test.ts"))
            .unwrap();
        assert!(got.ends_with("collections/slug.ts"));
    }
}
