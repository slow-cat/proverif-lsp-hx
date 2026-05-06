const PREC = {
  IMPLIES: 1,
  OR: 2,
  AND: 3,
  COMPARE: 4,
  ADD: 5,
  MULTIPLY: 6,
  UNARY: 7,
  CALL: 8,
  PREFIX: 9,
};

module.exports = grammar({
  name: "proverif",

  externals: $ => [
    $.comment,
  ],

  extras: $ => [
    /\s+/,
    $.comment,
  ],

  word: $ => $.identifier,

  supertypes: $ => [
    $._declaration,
    $._term,
    $._process,
  ],

  conflicts: $ => [
    [$._term, $.call_process],
    [$._term, $.binding],
    [$.tuple, $.tuple_pattern],
    [$.tuple, $.pattern],
    [$.binding, $.tuple_pattern],
    [$.binding, $.grouped_pattern],
    [$.pattern, $.binding],
    [$.in_process, $.binding],
    [$.new_name, $.binding],
    [$.arguments, $.pattern_arguments],
    [$.arguments, $.pattern],
    [$.query_expr, $.grouped_term],
    [$._term, $.binding, $.tuple_pattern],
    [$.sequenced_process, $.new_process],
    [$.sequenced_process, $.if_process],
    [$.sequenced_process, $.let_process],
    [$.sequenced_process, $.in_process],
    [$.sequenced_process, $.out_process],
    [$.sequenced_process, $.event_process],
    [$.sequenced_process, $.phase_process],
    [$.sequenced_process, $.call_process],
  ],

  rules: {
    source_file: $ => seq(
      repeat($._declaration),
      optional(choice($.process_decl, $.equivalence_decl))
    ),

    _declaration: $ => choice(
      $.type_decl,
      $.fun_decl,
      $.reduc_decl,
      $.equation_decl,
      $.clauses_decl,
      $.const_decl,
      $.free_decl,
      $.event_decl,
      $.pred_decl,
      $.table_decl,
      $.let_decl,
      $.letfun_decl,
      $.set_decl,
      $.nounif_decl,
      $.query_decl,
      $.noninterf_decl,
      $.weaksecret_decl,
      $.not_decl,
      $.lemma_decl,
      $.cryptoverif_decl,
      $.elimtrue_decl
    ),

    process_decl: $ => seq("process", field("body", $._process)),
    equivalence_decl: $ => seq(
      "equivalence",
      repeat1(choice($.raw_token, $.string))
    ),

    type_decl: $ => seq("type", field("name", $.identifier), optional($.options), "."),

    fun_decl: $ => seq(
      "fun",
      field("name", $.identifier),
      field("parameters", $.parameter_types),
      ":",
      field("result", $.type_expr),
      optional($.options),
      "."
    ),

    reduc_decl: $ => seq(
      "reduc",
      $.rule_clause,
      repeat(seq(";", $.rule_clause)),
      optional($.options),
      "."
    ),
    equation_decl: $ => seq(
      "equation",
      $.rule_clause,
      repeat(seq(";", $.rule_clause)),
      "."
    ),
    clauses_decl: $ => seq("clauses", repeat1($.clauses_clause)),
    clauses_clause: $ => seq(
      optional($.forall_clause),
      repeat1($.raw_token),
      choice(";", ".")
    ),

    const_decl: $ => seq(
      "const",
      field("name", $.identifier_list),
      ":",
      field("type", $.type_expr),
      optional($.options),
      "."
    ),

    free_decl: $ => seq(
      "free",
      field("name", $.identifier_list),
      ":",
      field("type", $.type_expr),
      optional($.options),
      "."
    ),

    event_decl: $ => seq(
      "event",
      field("name", $.identifier),
      optional($.parameter_types),
      "."
    ),

    pred_decl: $ => seq(
      "pred",
      field("name", $.identifier),
      optional($.parameter_types),
      optional($.options),
      "."
    ),

    table_decl: $ => seq(
      "table",
      field("name", $.identifier),
      $.parameter_types,
      "."
    ),

    let_decl: $ => seq(
      "let",
      field("name", $.identifier),
      optional($.binder_parameters),
      "=",
      field("body", $._process),
      "."
    ),

    letfun_decl: $ => seq(
      "letfun",
      field("name", $.identifier),
      optional($.binder_parameters),
      "=",
      field("body", $._term),
      "."
    ),

    set_decl: $ => seq(
      "set",
      field("name", $.identifier),
      "=",
      field("value", $.setting_value),
      "."
    ),

    nounif_decl: $ => seq(
      choice("nounif", "noselect", "select"),
      repeat1($.raw_token),
      "."
    ),

    query_decl: $ => seq(
      "query",
      optional(seq(field("bindings", $.query_bindings), ";")),
      field("body", $.query_sequence),
      optional($.options),
      "."
    ),
    noninterf_decl: $ => seq(
      "noninterf",
      optional(seq(field("bindings", $.query_bindings), ";")),
      field("body", $.noninterf_sequence),
      "."
    ),
    weaksecret_decl: $ => seq("weaksecret", field("body", $.identifier_list), "."),
    not_decl: $ => seq(
      "not",
      optional(seq(field("bindings", $.query_bindings), ";")),
      field("body", $.query_expr),
      "."
    ),
    lemma_decl: $ => seq(
      choice("lemma", "axiom", "restriction"),
      optional(seq(field("bindings", $.query_bindings), ";")),
      field("body", $.query_sequence),
      optional($.options),
      "."
    ),

    cryptoverif_decl: $ => seq(
      choice("param", "proba", "proof", "letproba", "implementation"),
      repeat($.raw_token),
      "."
    ),

    elimtrue_decl: $ => seq("elimtrue", repeat1($.raw_token), "."),

    rule_clause: $ => seq(
      optional($.forall_clause),
      field("body", $.query_expr),
      optional($.otherwise_clause)
    ),

    forall_clause: $ => seq("forall", commaSep1($.binding), ";"),
    otherwise_clause: $ => seq("otherwise", field("body", $.query_expr)),
    query_bindings: $ => commaSep1($.binding),
    query_sequence: $ => seq(
      $.query_expr,
      repeat(seq(";", $.query_expr))
    ),
    noninterf_sequence: $ => seq(
      $.noninterf_item,
      repeat(seq(",", $.noninterf_item))
    ),
    noninterf_item: $ => prec(1, seq(
      field("name", $.identifier),
      optional(seq("among", "(", field("values", $.term_sequence), ")"))
    )),
    term_sequence: $ => seq(
      $._term,
      repeat(seq(",", $._term))
    ),

    query_expr: $ => choice(
      prec.right(PREC.IMPLIES, seq(field("left", $.query_expr), "==>", field("right", $.query_expr))),
      prec.right(PREC.IMPLIES, seq(field("left", $.query_expr), "<=>", field("right", $.query_expr))),
      prec.left(PREC.AND, seq(field("left", $.query_expr), "&&", field("right", $.query_expr))),
      prec.left(PREC.OR, seq(field("left", $.query_expr), "||", field("right", $.query_expr))),
      $.grouped_query,
      $.prefix_query,
      $._term
    ),
    grouped_query: $ => seq("(", $.query_expr, ")"),

    prefix_query: $ => prec.right(PREC.PREFIX, choice(
      seq("event", $.event_call),
      seq("inj-event", $.event_call),
      seq("attacker", $.call_like_payload),
      seq("attacker", $.call_like_payload, "phase", $.number),
      seq("mess", $.call_like_payload),
      seq("mess", $.call_like_payload, "phase", $.number),
      seq("table", $.call_like_payload),
      seq("table", $.call_like_payload, "phase", $.number),
      seq("phase", $.number),
      seq("public_vars", $.identifier_list),
      seq("putbegin", $.identifier_list),
      seq("putbegin", choice("event", "inj-event"), ":", $.identifier_list),
      seq("secret", $.identifier_list)
    )),

    event_call: $ => seq("(", optional(commaSep1($._term)), ")"),
    call_like_payload: $ => seq("(", optional(commaSep1($._term)), ")"),

    _process: $ => choice(
      $.parallel_process,
      $.replicated_process,
      $.sequenced_process,
      $.nil_process,
      $.new_process,
      $.in_process,
      $.out_process,
      $.insert_process,
      $.get_process,
      $.if_process,
      $.let_process,
      $.event_process,
      $.phase_process,
      $.call_process,
      $.grouped_process
    ),

    grouped_process: $ => seq("(", $._process, ")"),
    parallel_process: $ => prec.left(PREC.OR, seq(field("left", $._process), "|", field("right", $._process))),
    replicated_process: $ => prec(PREC.PREFIX, seq("!", field("body", $._process))),
    sequenced_process: $ => prec.left(PREC.AND, seq(field("left", $._process), ";", field("right", $._process))),
    nil_process: $ => choice("0", "yield"),

    new_process: $ => seq("new", field("binding", $.new_binding), ";", field("body", $._process)),
    in_process: $ => seq(
      "in",
      "(",
      field("channel", $._term),
      ",",
      field("pattern", $.pattern),
      ")",
      optional($.options)
    ),
    out_process: $ => seq("out", "(", commaSep1($._term), ")"),
    insert_process: $ => seq(
      "insert",
      field("table", $.identifier),
      field("arguments", $.arguments)
    ),
    get_process: $ => prec.right(seq(
      "get",
      field("table", $.identifier),
      field("patterns", $.pattern_arguments),
      optional($.suchthat_clause),
      optional($.options),
      optional(seq("in", field("body", $._process))),
      optional(seq("else", field("alternative", $._process)))
    )),
    if_process: $ => prec.right(seq(
      "if",
      field("condition", $.query_expr),
      "then",
      field("consequence", $._process),
      optional(seq("else", field("alternative", $._process)))
    )),
    let_process: $ => prec.right(seq(
      "let",
      field("binding", $.pattern),
      "=",
      field("value", $._term),
      choice(
        seq("in", field("body", $._process), optional(seq("else", field("alternative", $._process)))),
        seq(optional(seq("else", field("alternative", $._process))))
      )
    )),
    event_process: $ => seq("event", field("value", choice($.call, $.event_call, $.identifier))),
    phase_process: $ => seq("phase", field("value", $.number)),
    call_process: $ => prec(PREC.CALL, seq(field("name", $.identifier), optional($.arguments))),

    _term: $ => choice(
      $.if_term,
      $.new_term,
      $.binary_expr,
      $.unary_expr,
      $.call,
      $.tuple,
      $.grouped_term,
      $.new_name,
      $.identifier,
      $.parameter,
      $.number,
      $.string,
      $.boolean,
      $.choice_term,
      $.fail_term
    ),

    grouped_term: $ => seq("(", $._term, ")"),
    tuple: $ => seq("(", commaSep2(choice($._term, $.binding)), ")"),
    if_term: $ => prec.right(seq(
      "if",
      field("condition", $.query_expr),
      "then",
      field("consequence", $._term),
      optional(seq("else", field("alternative", $._term)))
    )),
    new_term: $ => seq(
      "new",
      field("binding", $.new_binding),
      ";",
      field("body", $._term)
    ),
    call: $ => prec(PREC.CALL, seq(field("function", $.identifier), field("arguments", $.arguments))),
    new_name: $ => prec.right(seq(
      "new",
      field("name", $.identifier),
      optional(seq("[", optional($.name_binding_sequence), "]"))
    )),
    name_binding_sequence: $ => seq(
      $.name_binding,
      repeat(seq(";", $.name_binding))
    ),
    name_binding: $ => seq(
      field("name", $.identifier),
      "=",
      field("value", $._term)
    ),
    choice_term: $ => seq("choice", "[", field("left", $._term), ",", field("right", $._term), "]"),
    arguments: $ => seq("(", optional(commaSep1(choice($._term, $.binding))), ")"),
    pattern_arguments: $ => seq("(", optional(commaSep1($.pattern)), ")"),
    suchthat_clause: $ => seq("suchthat", field("condition", $._term)),

    unary_expr: $ => prec(PREC.UNARY, seq(
      field("operator", choice("not", "-", "+", "choice", "diff")),
      field("operand", $._term)
    )),

    binary_expr: $ => choice(
      prec.left(PREC.MULTIPLY, seq(field("left", $._term), field("operator", choice("*", "/")), field("right", $._term))),
      prec.left(PREC.ADD, seq(field("left", $._term), field("operator", choice("+", "-")), field("right", $._term))),
      prec.left(PREC.COMPARE, seq(field("left", $._term), field("operator", choice("=", "<>", "<", ">", "<=", ">=")), field("right", $._term))),
      prec.left(PREC.AND, seq(field("left", $._term), field("operator", "&&"), field("right", $._term))),
      prec.left(PREC.OR, seq(field("left", $._term), field("operator", "||"), field("right", $._term)))
    ),

    pattern: $ => choice(
      $.binding,
      $.call_pattern,
      $.grouped_pattern,
      $.tuple_pattern,
      $.wildcard_pattern,
      $.equality_pattern
    ),

    binding: $ => seq(
      field("name", choice($.identifier, $.tuple_pattern, $.wildcard_pattern)),
      optional(seq(":", field("type", $.type_expr))),
      optional(seq("or", "fail"))
    ),
    new_binding: $ => seq(
      field("name", $.identifier),
      optional(seq("[", optional($.identifier_list), "]")),
      ":",
      field("type", $.type_expr)
    ),

    grouped_pattern: $ => seq("(", $.pattern, ")"),
    call_pattern: $ => prec(PREC.CALL, seq(
      field("function", $.identifier),
      field("arguments", $.pattern_arguments)
    )),

    tuple_pattern: $ => seq("(", $.pattern, ",", $.pattern, repeat(seq(",", $.pattern)), ")"),
    wildcard_pattern: $ => "_",
    equality_pattern: $ => seq("=", field("value", $._term)),

    parameter_types: $ => seq("(", optional(commaSep1($.type_expr)), ")"),
    binder_parameters: $ => seq("(", optional(commaSep1($.binding)), ")"),
    identifier_list: $ => prec.right(seq($.identifier, repeat(seq(",", $.identifier)))),

    type_expr: $ => choice(
      $.identifier,
      $.call,
      $.grouped_type,
      prec.left(seq(field("left", $.type_expr), "*", field("right", $.type_expr)))
    ),
    grouped_type: $ => seq("(", $.type_expr, ")"),

    options: $ => seq("[", optional(commaSep1($.identifier)), "]"),
    setting_value: $ => choice($.boolean, $.number, $.identifier, $.string),
    operator_token: _ => token(choice("->", "<-", "<>", "&&", "||", "==>", "<=>", "<=", ">=", "!")),
    raw_token: _ => token(choice(
      /[A-Za-z_][A-Za-z0-9_'-]*/,
      /[0-9]+/,
      /@[A-Za-z_][A-Za-z0-9_']*/,
      "<-",
      "==>",
      "<=>",
      "<=",
      ">=",
      "<>",
      "&&",
      "||",
      "->",
      "|",
      "!",
      "/",
      ":",
      ";",
      ",",
      "=",
      "(",
      ")",
      "[",
      "]",
      "{",
      "}"
    )),

    boolean: _ => choice("true", "false"),
    fail_term: _ => "fail",
    identifier: _ => /[A-Za-z_][A-Za-z0-9_']*/,
    parameter: _ => /@[A-Za-z_][A-Za-z0-9_']*/,
    number: _ => /[0-9]+/,
    string: _ => /"([^"\\]|\\.)*"/,
  },
});

function commaSep1(rule) {
  return seq(rule, repeat(seq(",", rule)));
}

function commaSep2(rule) {
  return seq(rule, ",", rule, repeat(seq(",", rule)));
}
