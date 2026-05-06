(comment) @comment

[
  "type"
  "fun"
  "reduc"
  "equation"
  "const"
  "free"
  "event"
  "pred"
  "table"
  "let"
  "letfun"
  "set"
  "nounif"
  "noselect"
  "select"
  "query"
  "noninterf"
  "weaksecret"
  "not"
  "param"
  "proba"
  "proof"
  "letproba"
  "implementation"
  "elimtrue"
  "forall"
  "otherwise"
  "new"
  "in"
  "out"
  "if"
  "then"
  "else"
  "phase"
  "process"
  "inj-event"
  "attacker"
  "mess"
  "public_vars"
  "putbegin"
  "secret"
] @keyword

[
  "true"
  "false"
] @constant

[
  "!"
  "==>"
  "<=>"
  "&&"
  "||"
  "="
  "<>"
  "<"
  ">"
  "<="
  ">="
  "+"
  "-"
  "*"
  "/"
] @operator

[
  "("
  ")"
  "["
  "]"
] @punctuation.bracket

[
  ","
  ";"
  ":"
  "."
  "|"
] @punctuation.delimiter

(number) @constant.numeric
(string) @string
(parameter) @variable.parameter
(boolean) @constant
(fail_term) @constant

(type_decl
  name: (identifier) @type)

(type_expr
  (identifier) @type)

(binding
  type: (type_expr
    (identifier) @type))

(parameter_types
  (type_expr
    (identifier) @type))

(fun_decl
  name: (identifier) @function)

(event_decl
  name: (identifier) @function)

(pred_decl
  name: (identifier) @function)

(table_decl
  name: (identifier) @function)

(let_decl
  name: (identifier) @function)

(letfun_decl
  name: (identifier) @function)

(call
  function: (identifier) @function)

(call_process
  name: (identifier) @function)

(event_process
  value: (identifier) @function)

(const_decl
  name: (identifier_list (identifier) @constant))

(free_decl
  name: (identifier_list (identifier) @constant))

(options
  (identifier) @constant)

(set_decl
  name: (identifier) @constant)

(binding
  name: (identifier) @variable.parameter)

(forall_clause
  (binding
    name: (identifier) @variable.parameter))

(binder_parameters
  (binding
    name: (identifier) @variable.parameter))

(new_process
  binding: (new_binding
    name: (identifier) @variable.parameter))

(let_process
  binding: (pattern
    (binding
      name: (identifier) @variable.parameter)))

(in_process
  channel: (identifier) @variable)

(query_decl
  "query" @keyword)

(query_expr
  "==>" @operator)

(query_expr
  "<=>" @operator)

(prefix_query
  "inj-event" @keyword)

(prefix_query
  "attacker" @keyword)

(prefix_query
  "mess" @keyword)

(prefix_query
  "public_vars" @keyword)

(prefix_query
  "putbegin" @keyword)

(prefix_query
  "secret" @keyword)

(binary_expr
  operator: _ @operator)

(unary_expr
  operator: _ @operator)
