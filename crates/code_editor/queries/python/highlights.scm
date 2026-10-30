; Python highlights for tree-sitter-python, adapted from the grammar's
; queries/highlights.scm with VS Code Dark+ style token choices. Narrower
; captures override wider ones, and for the same range a later pattern
; overrides an earlier one.

; Identifiers

((identifier) @constant
 (#match? @constant "^[A-Z][A-Z0-9_]*$"))

(attribute attribute: (identifier) @property)

[
  (parameters (identifier) @variable.parameter)
  (lambda_parameters (identifier) @variable.parameter)
  (default_parameter name: (identifier) @variable.parameter)
  (typed_parameter (identifier) @variable.parameter)
  (typed_default_parameter name: (identifier) @variable.parameter)
  (list_splat_pattern (identifier) @variable.parameter)
  (dictionary_splat_pattern (identifier) @variable.parameter)
  (keyword_argument name: (identifier) @variable.parameter)
]

; Functions

(function_definition name: (identifier) @function)
(call function: (identifier) @function)
(call function: (attribute attribute: (identifier) @function))

; Types: class names, annotations, CapWords names and builtin types.

((identifier) @type
 (#match? @type "^_*[A-Z][A-Za-z0-9_]*[a-z][A-Za-z0-9_]*$"))

(class_definition name: (identifier) @type)
(type (identifier) @type)
(class_definition superclasses: (argument_list (identifier) @type))

((identifier) @type.builtin
 (#match? @type.builtin "^(bool|bytearray|bytes|complex|dict|float|frozenset|int|list|object|set|str|tuple|type)$"))

((identifier) @variable.builtin
 (#match? @variable.builtin "^(self|cls)$"))

(decorator) @decorator
(decorator (identifier) @decorator)
(decorator (attribute attribute: (identifier) @decorator))
(decorator (call function: (identifier) @decorator))

; Literals

[
  (none)
  (true)
  (false)
] @constant.builtin

[
  (integer)
  (float)
] @number

(comment) @comment
(string) @string
(escape_sequence) @string.escape

; Code inside f-string braces is not string-colored.
(interpolation
  "{" @punctuation
  expression: (_) @none
  "}" @punctuation)

(format_specifier) @string

; Operators and keywords

[
  "-"
  "-="
  "!="
  "*"
  "**"
  "**="
  "*="
  "/"
  "//"
  "//="
  "/="
  "&"
  "&="
  "%"
  "%="
  "^"
  "^="
  "+"
  "->"
  "+="
  "<"
  "<<"
  "<<="
  "<="
  "<>"
  "="
  ":="
  "=="
  ">"
  ">="
  ">>"
  ">>="
  "|"
  "|="
  "~"
  "@="
] @operator

[
  "and"
  "in"
  "is"
  "not"
  "or"
  "is not"
  "not in"
  "def"
  "class"
  "lambda"
  "async"
  "global"
  "nonlocal"
  "del"
  "assert"
  "exec"
  "print"
] @keyword

[
  "as"
  "await"
  "break"
  "continue"
  "elif"
  "else"
  "except"
  "finally"
  "for"
  "from"
  "if"
  "import"
  "pass"
  "raise"
  "return"
  "try"
  "while"
  "with"
  "yield"
  "match"
  "case"
] @keyword.control
