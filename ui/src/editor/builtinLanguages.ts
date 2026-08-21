// GENERATED FILE — DO NOT EDIT.
//
// Every type below comes from a `#[derive(TS)]` type in `crates/cide-ipc`, concatenated in
// type-name order. Regenerate with `cargo xtask codegen`.
//
// CI runs `cargo xtask codegen --check`, so a Rust field rename that never reached this
// file fails the build instead of surfacing as an `undefined` in the webview at runtime.

// Written by `cargo xtask codegen` from `cide_ipc::lang::builtins()`.
//
// A builtin's *tokenizer* is not here and cannot be: it contains a function, so it stays in
// `ui/src/editor/languages/<id>.ts` and is reached by the dynamic import in `languages.ts`.
// Everything else about a language — which extensions it claims, what the status bar calls it,
// how it folds, whether the scratch picker offers it — is a table, and a table that has to agree
// with the ones an extension contributes has to have exactly one home. This is it.
import type { LanguageDef, LanguageServerDef } from '../ipc/generated'

export const BUILTIN_LANGUAGES: readonly LanguageDef[] = [
  {
    "id": "rust",
    "label": "Rust",
    "extensions": [
      {
        "ext": "rs"
      }
    ],
    "filenames": [],
    "fenceAliases": [
      "rust"
    ],
    "grammar": {
      "name": "Rust",
      "keywords": [],
      "caseInsensitiveKeywords": false,
      "types": [],
      "atoms": [],
      "builtins": [],
      "nestedComments": false,
      "tripleQuotes": false,
      "capitalisedIsType": false,
      "callSyntax": false,
      "rules": []
    },
    "fold": {
      "lineComment": "//",
      "blockComment": [
        "/*",
        "*/"
      ],
      "nestedComments": true,
      "multilineQuotes": "\"",
      "lifetimes": true,
      "rawStrings": true,
      "indentBlocks": false,
      "headingFolds": false,
      "fencedBlocks": false,
      "regions": true
    },
    "scratch": [
      {
        "label": "Rust",
        "ext": "rs"
      }
    ]
  },
  {
    "id": "go",
    "label": "Go",
    "extensions": [
      {
        "ext": "go"
      }
    ],
    "filenames": [],
    "fenceAliases": [
      "golang"
    ],
    "grammar": {
      "name": "Go",
      "keywords": [],
      "caseInsensitiveKeywords": false,
      "types": [],
      "atoms": [],
      "builtins": [],
      "nestedComments": false,
      "tripleQuotes": false,
      "capitalisedIsType": false,
      "callSyntax": false,
      "rules": []
    },
    "fold": {
      "lineComment": "//",
      "blockComment": [
        "/*",
        "*/"
      ],
      "nestedComments": false,
      "quotes": "\"",
      "multilineQuotes": "`",
      "extraQuotes": "`",
      "lifetimes": false,
      "rawStrings": false,
      "indentBlocks": false,
      "headingFolds": false,
      "fencedBlocks": false,
      "regions": true
    },
    "scratch": [
      {
        "label": "Go",
        "ext": "go"
      }
    ]
  },
  {
    "id": "typescript",
    "label": "TypeScript",
    "extensions": [
      {
        "ext": "ts"
      },
      {
        "ext": "mts"
      },
      {
        "ext": "cts"
      },
      {
        "ext": "tsx",
        "label": "TSX"
      },
      {
        "ext": "js",
        "label": "JavaScript"
      },
      {
        "ext": "mjs",
        "label": "JavaScript"
      },
      {
        "ext": "cjs",
        "label": "JavaScript"
      },
      {
        "ext": "jsx",
        "label": "JSX"
      }
    ],
    "filenames": [],
    "fenceAliases": [
      "typescript",
      "javascript"
    ],
    "grammar": {
      "name": "TypeScript",
      "keywords": [],
      "caseInsensitiveKeywords": false,
      "types": [],
      "atoms": [],
      "builtins": [],
      "nestedComments": false,
      "tripleQuotes": false,
      "capitalisedIsType": false,
      "callSyntax": false,
      "rules": []
    },
    "fold": {
      "lineComment": "//",
      "blockComment": [
        "/*",
        "*/"
      ],
      "multilineQuotes": "`",
      "extraQuotes": "`",
      "lifetimes": false,
      "rawStrings": false,
      "indentBlocks": false,
      "headingFolds": false,
      "fencedBlocks": false,
      "regions": true
    },
    "scratch": [
      {
        "label": "TypeScript",
        "ext": "ts"
      },
      {
        "label": "JavaScript",
        "ext": "js"
      }
    ]
  },
  {
    "id": "python",
    "label": "Python",
    "extensions": [
      {
        "ext": "py"
      },
      {
        "ext": "pyi"
      }
    ],
    "filenames": [],
    "fenceAliases": [
      "python"
    ],
    "grammar": {
      "name": "Python",
      "keywords": [],
      "caseInsensitiveKeywords": false,
      "types": [],
      "atoms": [],
      "builtins": [],
      "nestedComments": false,
      "tripleQuotes": false,
      "capitalisedIsType": false,
      "callSyntax": false,
      "rules": []
    },
    "fold": {
      "lineComment": "#",
      "tripleQuotes": true,
      "lifetimes": false,
      "rawStrings": false,
      "indentBlocks": true,
      "headingFolds": false,
      "fencedBlocks": false,
      "regions": true
    },
    "scratch": [
      {
        "label": "Python",
        "ext": "py"
      }
    ]
  },
  {
    "id": "json",
    "label": "JSON",
    "extensions": [
      {
        "ext": "json"
      },
      {
        "ext": "jsonc"
      }
    ],
    "filenames": [],
    "fenceAliases": [
      "jsonc"
    ],
    "grammar": {
      "name": "JSON",
      "keywords": [],
      "caseInsensitiveKeywords": false,
      "types": [],
      "atoms": [],
      "builtins": [],
      "nestedComments": false,
      "tripleQuotes": false,
      "capitalisedIsType": false,
      "callSyntax": false,
      "rules": []
    },
    "fold": {
      "lineComment": "//",
      "blockComment": [
        "/*",
        "*/"
      ],
      "quotes": "\"",
      "lifetimes": false,
      "rawStrings": false,
      "indentBlocks": false,
      "headingFolds": false,
      "fencedBlocks": false,
      "regions": false
    },
    "scratch": [
      {
        "label": "JSON",
        "ext": "json"
      }
    ]
  },
  {
    "id": "yaml",
    "label": "YAML",
    "extensions": [
      {
        "ext": "yaml"
      },
      {
        "ext": "yml"
      }
    ],
    "filenames": [],
    "fenceAliases": [
      "yml"
    ],
    "grammar": {
      "name": "YAML",
      "keywords": [],
      "caseInsensitiveKeywords": false,
      "types": [],
      "atoms": [],
      "builtins": [],
      "nestedComments": false,
      "tripleQuotes": false,
      "capitalisedIsType": false,
      "callSyntax": false,
      "rules": []
    },
    "fold": {
      "lineComment": "#",
      "quotes": "\"'",
      "lifetimes": false,
      "rawStrings": false,
      "indentBlocks": true,
      "headingFolds": false,
      "fencedBlocks": false,
      "regions": true
    },
    "scratch": [
      {
        "label": "YAML",
        "ext": "yaml"
      }
    ]
  },
  {
    "id": "toml",
    "label": "TOML",
    "extensions": [
      {
        "ext": "toml"
      },
      {
        "ext": "lock",
        "label": "TOML"
      }
    ],
    "filenames": [
      ".gitconfig"
    ],
    "fenceAliases": [],
    "grammar": {
      "name": "TOML",
      "keywords": [],
      "caseInsensitiveKeywords": false,
      "types": [],
      "atoms": [],
      "builtins": [],
      "nestedComments": false,
      "tripleQuotes": false,
      "capitalisedIsType": false,
      "callSyntax": false,
      "rules": []
    },
    "fold": {
      "lineComment": "#",
      "quotes": "\"'",
      "tripleQuotes": true,
      "lifetimes": false,
      "rawStrings": false,
      "indentBlocks": false,
      "headingFolds": false,
      "fencedBlocks": false,
      "regions": true
    },
    "scratch": [
      {
        "label": "TOML",
        "ext": "toml"
      }
    ]
  },
  {
    "id": "markdown",
    "label": "Markdown",
    "extensions": [
      {
        "ext": "md"
      },
      {
        "ext": "markdown"
      }
    ],
    "filenames": [],
    "fenceAliases": [],
    "grammar": {
      "name": "Markdown",
      "keywords": [],
      "caseInsensitiveKeywords": false,
      "types": [],
      "atoms": [],
      "builtins": [],
      "nestedComments": false,
      "tripleQuotes": false,
      "capitalisedIsType": false,
      "callSyntax": false,
      "rules": []
    },
    "fold": {
      "quotes": "",
      "brackets": "",
      "lifetimes": false,
      "rawStrings": false,
      "indentBlocks": false,
      "headingFolds": true,
      "fencedBlocks": true,
      "regions": false
    },
    "scratch": [
      {
        "label": "Markdown",
        "ext": "md"
      }
    ]
  },
  {
    "id": "shell",
    "label": "Shell",
    "extensions": [
      {
        "ext": "sh"
      },
      {
        "ext": "bash"
      },
      {
        "ext": "zsh"
      },
      {
        "ext": "fish"
      }
    ],
    "filenames": [
      ".bashrc",
      ".zshrc",
      ".profile",
      "dockerfile",
      "makefile"
    ],
    "fenceAliases": [
      "shell",
      "console",
      "shell-session",
      "sh-session",
      "zsh"
    ],
    "grammar": {
      "name": "Shell",
      "keywords": [],
      "caseInsensitiveKeywords": false,
      "types": [],
      "atoms": [],
      "builtins": [],
      "nestedComments": false,
      "tripleQuotes": false,
      "capitalisedIsType": false,
      "callSyntax": false,
      "rules": []
    },
    "fold": {
      "lineComment": "#",
      "multilineQuotes": "\"'",
      "lifetimes": false,
      "rawStrings": false,
      "indentBlocks": false,
      "headingFolds": false,
      "fencedBlocks": false,
      "regions": true
    },
    "scratch": [
      {
        "label": "Shell",
        "ext": "sh"
      }
    ]
  },
  {
    "id": "sql",
    "label": "SQL",
    "extensions": [
      {
        "ext": "sql"
      }
    ],
    "filenames": [],
    "fenceAliases": [],
    "grammar": {
      "name": "SQL",
      "keywords": [],
      "caseInsensitiveKeywords": false,
      "types": [],
      "atoms": [],
      "builtins": [],
      "nestedComments": false,
      "tripleQuotes": false,
      "capitalisedIsType": false,
      "callSyntax": false,
      "rules": []
    },
    "fold": {
      "lineComment": "--",
      "blockComment": [
        "/*",
        "*/"
      ],
      "nestedComments": false,
      "quotes": "'\"`",
      "escapes": false,
      "multilineQuotes": "'",
      "lifetimes": false,
      "rawStrings": false,
      "indentBlocks": false,
      "headingFolds": false,
      "fencedBlocks": false,
      "regions": true
    },
    "scratch": [
      {
        "label": "SQL",
        "ext": "sql"
      }
    ]
  },
  {
    "id": "clike",
    "label": "C-like",
    "extensions": [
      {
        "ext": "c",
        "label": "C"
      },
      {
        "ext": "h",
        "label": "C"
      },
      {
        "ext": "cc",
        "label": "C++"
      },
      {
        "ext": "cpp",
        "label": "C++"
      },
      {
        "ext": "cxx",
        "label": "C++"
      },
      {
        "ext": "hpp",
        "label": "C++"
      },
      {
        "ext": "java",
        "label": "Java"
      }
    ],
    "filenames": [],
    "fenceAliases": [
      "c++",
      "objc"
    ],
    "grammar": {
      "name": "C-like",
      "keywords": [],
      "caseInsensitiveKeywords": false,
      "types": [],
      "atoms": [],
      "builtins": [],
      "nestedComments": false,
      "tripleQuotes": false,
      "capitalisedIsType": false,
      "callSyntax": false,
      "rules": []
    },
    "fold": {
      "lineComment": "//",
      "blockComment": [
        "/*",
        "*/"
      ],
      "lifetimes": false,
      "rawStrings": false,
      "indentBlocks": false,
      "headingFolds": false,
      "fencedBlocks": false,
      "regions": true
    },
    "scratch": [
      {
        "label": "C",
        "ext": "c"
      },
      {
        "label": "C++",
        "ext": "cpp"
      }
    ]
  }
]

export const BUILTIN_SERVERS: readonly LanguageServerDef[] = [
  {
    "binary": "rust-analyzer",
    "args": [],
    "languageIds": [
      "rust"
    ],
    "projectMarkers": [
      "Cargo.toml"
    ],
    "projectKind": "Cargo",
    "installHint": "rustup component add rust-analyzer",
    "declaresWatchedFiles": false,
    "extraPathHints": [
      "~/.cargo/bin"
    ]
  },
  {
    "binary": "gopls",
    "args": [],
    "languageIds": [
      "go"
    ],
    "projectMarkers": [
      "go.mod",
      "go.work"
    ],
    "projectKind": "Go module",
    "installHint": "go install golang.org/x/tools/gopls@latest",
    "declaresWatchedFiles": true,
    "extraPathHints": [
      "~/go/bin"
    ]
  }
]
