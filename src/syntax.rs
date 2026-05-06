#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use anyhow::{Context, Result};
use crate::keywords::KEYWORD_CANDIDATES;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, Diagnostic, DiagnosticSeverity, Position,
    Range as LspRange,
};
use tree_sitter::{
    Language, Node, Parser, Point, Query, QueryCapture, QueryCursor, StreamingIterator, Tree,
};

#[derive(Debug, Clone)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: String,
    pub range: Range<usize>,
    pub selection_range: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Type,
    Function,
    Constant,
    Variable,
}

const BUILTIN_TYPE_NAMES: &[&str] = &["bitstring", "channel", "bool", "nat"];
const BUILTIN_FUNCTION_NAMES: &[&str] = &["true", "false", "not"];
const FREE_NAME_OPTION_NAMES: &[&str] = &["private"];
const PROCESS_IO_OPTION_NAMES: &[&str] = &["precise"];
const QUERY_SECRET_OPTION_NAMES: &[&str] = &[
    "reachability",
    "pv_reachability",
    "real_or_random",
    "pv_real_or_random",
];

pub struct ParsedDocument {
    tree: Tree,
    source: String,
}

impl ParsedDocument {
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();
        collect_error_nodes(self.tree.root_node(), &self.source, &mut diagnostics);
        diagnostics.extend(self.semantic_diagnostics().unwrap_or_default());
        diagnostics
    }

    pub fn hover(&self, position: Position) -> Option<String> {
        let node = self.node_at(position)?;
        if node.kind() != "identifier" && node.kind() != "parameter" {
            return None;
        }

        let name = node.utf8_text(self.source.as_bytes()).ok()?;
        let parent = node.parent()?;
        let label = match parent.kind() {
            "type_decl" => "type",
            "fun_decl" | "event_decl" | "pred_decl" | "table_decl" | "let_decl" | "letfun_decl" => {
                "declaration"
            }
            "const_decl" | "free_decl" => "constant",
            "set_decl" => "setting",
            _ => "identifier",
        };

        Some(format!("`{name}` ({label})"))
    }

    pub fn symbols(&self) -> Result<Vec<Symbol>> {
        let query = Query::new(
            &language(),
            r#"
            (type_decl name: (identifier) @type)
            (fun_decl name: (identifier) @function)
            (event_decl name: (identifier) @function)
            (pred_decl name: (identifier) @function)
            (table_decl name: (identifier) @function)
            (let_decl name: (identifier) @function)
            (letfun_decl name: (identifier) @function)
            (set_decl name: (identifier) @variable)
            (const_decl name: (identifier_list (identifier) @constant))
            (free_decl name: (identifier_list (identifier) @constant))
            "#,
        )
        .context("failed to compile syntax symbol query")?;

        let mut cursor = QueryCursor::new();
        let capture_names = query.capture_names();
        let mut symbols = Vec::new();

        let mut matches = cursor.matches(&query, self.tree.root_node(), self.source.as_bytes());
        while let Some(query_match) = matches.next() {
            for QueryCapture { node, index } in query_match.captures.iter().copied() {
                let Some(name) = node.utf8_text(self.source.as_bytes()).ok() else {
                    continue;
                };
                let kind = match capture_names[index as usize] {
                    "type" => SymbolKind::Type,
                    "function" => SymbolKind::Function,
                    "constant" => SymbolKind::Constant,
                    "variable" => SymbolKind::Variable,
                    _ => continue,
                };

                symbols.push(Symbol {
                    kind,
                    name: name.to_owned(),
                    range: node.byte_range(),
                    selection_range: node.byte_range(),
                });
            }
        }

        Ok(symbols)
    }

    pub fn symbol_at(&self, position: Position) -> Option<Symbol> {
        let offset = offset_for_position(&self.source, position)?;
        self.symbols().ok()?.into_iter().find(|symbol| {
            symbol.selection_range.start <= offset && offset <= symbol.selection_range.end
        })
    }

    pub fn completion_items(&self, position: Position) -> Vec<CompletionItem> {
        let prefix = completion_prefix(&self.source, position).unwrap_or_default();
        let mut labels = HashSet::new();
        let mut items = Vec::new();

        if let Some(node) = self.node_at(position) {
            let state = offset_for_position(&self.source, position)
                .map(|offset| {
                    if node.start_byte() <= offset && offset < node.end_byte() {
                        node.parse_state()
                    } else {
                        node.next_parse_state()
                    }
                })
                .unwrap_or_else(|| node.next_parse_state());
            if let Some(mut lookahead) = language().lookahead_iterator(state) {
                for symbol_name in lookahead.iter_names() {
                    if symbol_name == "identifier" {
                        continue;
                    }
                    if !KEYWORD_CANDIDATES.contains(&symbol_name) {
                        continue;
                    }
                    if !matches_prefix(symbol_name, &prefix) || !labels.insert(symbol_name.to_owned()) {
                        continue;
                    }
                    items.push(CompletionItem {
                        label: symbol_name.to_owned(),
                        kind: Some(CompletionItemKind::KEYWORD),
                        ..CompletionItem::default()
                    });
                }
            }
        }

        for option in self.option_candidates_at_position(position) {
            if !matches_prefix(option, &prefix) || !labels.insert(option.to_owned()) {
                continue;
            }
            items.push(CompletionItem {
                label: option.to_owned(),
                kind: Some(CompletionItemKind::KEYWORD),
                detail: Some("option".to_owned()),
                ..CompletionItem::default()
            });
        }

        for (name, kind, detail) in self.completion_symbol_entries() {
            if !matches_prefix(&name, &prefix) || !labels.insert(name.clone()) {
                continue;
            }
            items.push(CompletionItem {
                label: name,
                kind: Some(kind),
                detail: Some(detail.to_owned()),
                ..CompletionItem::default()
            });
        }

        if items.is_empty() {
            for keyword in KEYWORD_CANDIDATES {
                if !matches_prefix(keyword, &prefix) || !labels.insert((*keyword).to_owned()) {
                    continue;
                }
                items.push(CompletionItem {
                    label: (*keyword).to_owned(),
                    kind: Some(CompletionItemKind::KEYWORD),
                    ..CompletionItem::default()
                });
            }
        }

        items.sort_by(|left, right| {
            completion_item_priority(left)
                .cmp(&completion_item_priority(right))
                .then_with(|| left.label.cmp(&right.label))
        });
        items
    }

    fn option_candidates_at_position(&self, position: Position) -> Vec<&'static str> {
        let Some(mut node) = self.node_at(position) else {
            return Vec::new();
        };
        loop {
            if node.kind() == "options" {
                let Some(owner) = node.parent() else {
                    return Vec::new();
                };
                return option_policy_for_owner(owner, &self.source)
                    .map(|policy| policy.candidates().to_vec())
                    .unwrap_or_default();
            }
            let Some(parent) = node.parent() else {
                break;
            };
            node = parent;
        }
        Vec::new()
    }

    fn completion_symbol_entries(&self) -> Vec<(String, CompletionItemKind, &'static str)> {
        let mut entries = Vec::new();
        let mut labels = HashSet::new();
        let globals = self.globals_for_completion();

        for name in globals.types {
            if labels.insert(name.clone()) {
                entries.push((name, CompletionItemKind::CLASS, "type"));
            }
        }
        for (name, _) in globals.functions {
            if labels.insert(name.clone()) {
                entries.push((name, CompletionItemKind::FUNCTION, "function"));
            }
        }
        for (name, _) in globals.events {
            if labels.insert(name.clone()) {
                entries.push((name, CompletionItemKind::FUNCTION, "event"));
            }
        }
        for (name, _) in globals.predicates {
            if labels.insert(name.clone()) {
                entries.push((name, CompletionItemKind::FUNCTION, "predicate"));
            }
        }
        for (name, _) in globals.tables {
            if labels.insert(name.clone()) {
                entries.push((name, CompletionItemKind::FUNCTION, "table"));
            }
        }
        for (name, _) in globals.processes {
            if labels.insert(name.clone()) {
                entries.push((name, CompletionItemKind::FUNCTION, "process"));
            }
        }
        for (name, _) in globals.names {
            if labels.insert(name.clone()) {
                entries.push((name, CompletionItemKind::CONSTANT, "constant"));
            }
        }
        for name in self.completion_variable_names() {
            if labels.insert(name.clone()) {
                entries.push((name, CompletionItemKind::VARIABLE, "variable"));
            }
        }
        for symbol in self
            .symbols()
            .unwrap_or_default()
            .into_iter()
            .filter(|symbol| symbol.kind == SymbolKind::Variable)
        {
            if labels.insert(symbol.name.clone()) {
                entries.push((symbol.name, CompletionItemKind::VARIABLE, "variable"));
            }
        }

        entries
    }

    fn completion_variable_names(&self) -> Vec<String> {
        let query = match Query::new(
            &language(),
            r#"
            (binding name: (identifier) @variable)
            (new_binding name: (identifier) @variable)
            (name_binding name: (identifier) @variable)
            "#,
        ) {
            Ok(query) => query,
            Err(_) => return Vec::new(),
        };
        let mut cursor = QueryCursor::new();
        let capture_names = query.capture_names();
        let mut names = Vec::new();
        let mut seen = HashSet::new();

        let mut matches = cursor.matches(&query, self.tree.root_node(), self.source.as_bytes());
        while let Some(query_match) = matches.next() {
            for QueryCapture { node, index } in query_match.captures.iter().copied() {
                if capture_names[index as usize] != "variable" {
                    continue;
                }
                let Some(name) = node.utf8_text(self.source.as_bytes()).ok() else {
                    continue;
                };
                if seen.insert(name.to_owned()) {
                    names.push(name.to_owned());
                }
            }
        }

        names
    }

    fn globals_for_completion(&self) -> GlobalEnv {
        let mut checker = SemanticChecker::new(&self.source);
        let root = self.tree.root_node();
        checker.collect_types(root);
        checker.collect_globals(root);
        checker.globals
    }

    fn semantic_diagnostics(&self) -> Result<Vec<Diagnostic>> {
        Ok(SemanticChecker::new(&self.source).run(self.tree.root_node()))
    }

    fn node_at(&self, position: Position) -> Option<Node<'_>> {
        let point = Point {
            row: position.line as usize,
            column: position.character as usize,
        };
        self.tree
            .root_node()
            .descendant_for_point_range(point, point)
    }
}

pub fn parse(source: &str) -> Result<ParsedDocument> {
    let mut parser = Parser::new();
    parser
        .set_language(&language())
        .context("failed to load proverif tree-sitter language")?;

    let tree = parser
        .parse(source, None)
        .context("tree-sitter parser returned no tree")?;

    Ok(ParsedDocument {
        tree,
        source: source.to_owned(),
    })
}

fn collect_error_nodes(node: Node<'_>, source: &str, diagnostics: &mut Vec<Diagnostic>) {
    if node.is_error() || node.is_missing() {
        diagnostics.push(Diagnostic {
            range: range_from_node(source, node),
            severity: Some(DiagnosticSeverity::ERROR),
            message: format!("syntax error near `{}`", snippet_for_node(source, node)),
            ..Diagnostic::default()
        });
    }

    for child in node.children(&mut node.walk()) {
        collect_error_nodes(child, source, diagnostics);
    }
}

#[derive(Debug, Clone)]
struct CallableDecl {
    arity: usize,
    arg_types: Vec<Option<String>>,
    result_type: Option<String>,
    range: Range<usize>,
}

#[derive(Debug, Clone)]
struct ValueDecl {
    type_name: Option<String>,
    range: Range<usize>,
    is_free: bool,
    is_private: bool,
}

enum IdentifierLookup {
    Variable(Option<String>),
    Name(Option<String>),
    ZeroArgFunction(Option<String>),
    ZeroArgPredicate,
    Function { arity: usize },
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionPolicy {
    FreeName,
    ProcessInputGet,
    QuerySecret,
}

impl OptionPolicy {
    fn candidates(self) -> &'static [&'static str] {
        match self {
            OptionPolicy::FreeName => FREE_NAME_OPTION_NAMES,
            OptionPolicy::ProcessInputGet => PROCESS_IO_OPTION_NAMES,
            OptionPolicy::QuerySecret => QUERY_SECRET_OPTION_NAMES,
        }
    }

    fn allows(self, name: &str) -> bool {
        match self {
            OptionPolicy::QuerySecret => {
                QUERY_SECRET_OPTION_NAMES.contains(&name) || name.starts_with("cv_")
            }
            _ => self.candidates().contains(&name),
        }
    }

    fn error_message(self) -> &'static str {
        match self {
            OptionPolicy::FreeName => "for free names, the only allowed option is private",
            OptionPolicy::ProcessInputGet => {
                "process input and get can only have \"precise\" as option"
            }
            OptionPolicy::QuerySecret => "the allowed options for query secret are reachability, pv_reachability, real_or_random, pv_real_or_random, and options starting with cv_",
        }
    }
}

#[derive(Default)]
struct GlobalEnv {
    functions: HashMap<String, CallableDecl>,
    processes: HashMap<String, CallableDecl>,
    events: HashMap<String, CallableDecl>,
    predicates: HashMap<String, CallableDecl>,
    tables: HashMap<String, CallableDecl>,
    names: HashMap<String, ValueDecl>,
    reduc_functions: HashSet<String>,
    types: HashSet<String>,
}

type LocalEnv = HashMap<String, Option<String>>;

struct SemanticChecker<'a> {
    source: &'a str,
    globals: GlobalEnv,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> SemanticChecker<'a> {
    fn new(source: &'a str) -> Self {
        let mut globals = GlobalEnv::default();
        globals
            .types
            .extend(BUILTIN_TYPE_NAMES.iter().map(|name| (*name).to_owned()));

        globals.functions.insert(
            "true".into(),
            CallableDecl {
                arity: 0,
                arg_types: Vec::new(),
                result_type: Some("bool".into()),
                range: 0..0,
            },
        );
        globals.functions.insert(
            "false".into(),
            CallableDecl {
                arity: 0,
                arg_types: Vec::new(),
                result_type: Some("bool".into()),
                range: 0..0,
            },
        );
        globals.functions.insert(
            "not".into(),
            CallableDecl {
                arity: 1,
                arg_types: vec![Some("bool".into())],
                result_type: Some("bool".into()),
                range: 0..0,
            },
        );

        Self {
            source,
            globals,
            diagnostics: Vec::new(),
        }
    }

    fn run(mut self, root: Node<'_>) -> Vec<Diagnostic> {
        self.collect_types(root);
        self.collect_globals(root);
        self.check_top_level(root);
        self.diagnostics
    }

    fn collect_types(&mut self, root: Node<'_>) {
        for node in root.named_children(&mut root.walk()) {
            if node.kind() != "type_decl" {
                continue;
            }
            if let Some(name_node) = node.child_by_field_name("name") {
                if let Some(name) = node_text(self.source, name_node) {
                    if self.name_defined_elsewhere(&name, Some(SymbolKindDecl::Type)) {
                        self.diagnostics.push(Diagnostic {
                            range: range_from_node(self.source, name_node),
                            severity: Some(DiagnosticSeverity::WARNING),
                            message: format!("identifier `{name}` already defined"),
                            ..Diagnostic::default()
                        });
                    }
                    self.globals.types.insert(name);
                }
            }
        }
    }

    fn collect_globals(&mut self, root: Node<'_>) {
        for node in root.named_children(&mut root.walk()) {
            match node.kind() {
                "type_decl" => {}
                "fun_decl" => {
                    if let Some(parameters) = node.child_by_field_name("parameters") {
                        self.validate_type_nodes(parameters);
                    }
                    if let Some(result) = node.child_by_field_name("result") {
                        self.validate_type_nodes(result);
                    }
                    self.validate_fun_decl(node);
                    self.insert_callable(
                        SymbolKindRef::Function,
                        node.child_by_field_name("name"),
                        parameter_types(node.child_by_field_name("parameters"), self.source),
                        node.child_by_field_name("result")
                            .map(|n| type_text(self.source, n)),
                    );
                }
                "letfun_decl" => {
                    self.validate_binder_parameters(node);
                    self.insert_callable(
                        SymbolKindRef::Function,
                        node.child_by_field_name("name"),
                        binder_parameter_types(node, self.source),
                        None,
                    );
                }
                "let_decl" => {
                    self.validate_binder_parameters(node);
                    self.insert_callable(
                        SymbolKindRef::Process,
                        node.child_by_field_name("name"),
                        binder_parameter_types(node, self.source),
                        None,
                    );
                }
                "event_decl" => {
                    if let Some(parameters) = first_named_child_of_kind(node, "parameter_types") {
                        self.validate_type_nodes(parameters);
                    }
                    self.insert_callable(
                        SymbolKindRef::Event,
                        node.child_by_field_name("name"),
                        parameter_types(first_named_child_of_kind(node, "parameter_types"), self.source),
                        None,
                    );
                }
                "pred_decl" => {
                    if let Some(parameters) = first_named_child_of_kind(node, "parameter_types") {
                        self.validate_type_nodes(parameters);
                    }
                    self.insert_extra_callable(
                        SymbolKindDecl::Predicate,
                        node.child_by_field_name("name"),
                        parameter_types(first_named_child_of_kind(node, "parameter_types"), self.source),
                        Some("bool".into()),
                    );
                }
                "table_decl" => {
                    if let Some(parameters) = first_named_child_of_kind(node, "parameter_types") {
                        self.validate_type_nodes(parameters);
                    }
                    self.insert_extra_callable(
                        SymbolKindDecl::Table,
                        node.child_by_field_name("name"),
                        parameter_types(first_named_child_of_kind(node, "parameter_types"), self.source),
                        None,
                    );
                }
                "free_decl" | "const_decl" => {
                    if let Some(ty_node) = node.child_by_field_name("type") {
                        self.validate_type_nodes(ty_node);
                    }
                    self.validate_options_for_owner(node);
                    self.validate_name_decl(node);
                    let ty = node
                        .child_by_field_name("type")
                        .map(|n| type_text(self.source, n));
                    if let Some(list) = node.child_by_field_name("name") {
                        let is_free = node.kind() == "free_decl";
                        let is_private = is_free
                            && first_named_child_of_kind(node, "options")
                                .map(|options| {
                                    options
                                        .named_children(&mut options.walk())
                                        .filter_map(|child| node_text(self.source, child))
                                        .any(|name| name == "private")
                                })
                                .unwrap_or(false);
                        for ident in list.named_children(&mut list.walk()) {
                            if ident.kind() != "identifier" {
                                continue;
                            }
                            if let Some(name) = node_text(self.source, ident) {
                                if self.name_defined_elsewhere(&name, Some(SymbolKindDecl::Name)) {
                                    self.diagnostics.push(Diagnostic {
                                        range: range_from_node(self.source, ident),
                                        severity: Some(DiagnosticSeverity::WARNING),
                                        message: format!("identifier `{name}` already defined"),
                                        ..Diagnostic::default()
                                    });
                                }
                                self.globals.names.insert(
                                    name,
                                    ValueDecl {
                                        type_name: ty.clone(),
                                        range: ident.byte_range(),
                                        is_free,
                                        is_private,
                                    },
                                );
                            }
                        }
                    }
                }
                "reduc_decl" => self.collect_reduc_decl(node),
                _ => {}
            }
        }
    }

    fn collect_reduc_decl(&mut self, reduc: Node<'_>) {
        for clause in reduc.named_children(&mut reduc.walk()) {
            if clause.kind() != "rule_clause" {
                continue;
            }
            let Some(body) = clause.child_by_field_name("body") else {
                continue;
            };
            let Some(binary) = first_named_descendant_of_kind(body, "binary_expr") else {
                continue;
            };
            let Some(left) = binary.child_by_field_name("left") else {
                continue;
            };
            if left.kind() != "call" {
                continue;
            }
            let Some(name_node) = left.child_by_field_name("function") else {
                continue;
            };
            let arg_types = left
                .child_by_field_name("arguments")
                .map(|args| vec![None; count_call_arguments(args)])
                .unwrap_or_default();
            if let Some(name) = node_text(self.source, name_node) {
                self.globals.reduc_functions.insert(name);
            }
            self.insert_callable(SymbolKindRef::Function, Some(name_node), arg_types, None);
        }
    }

    fn insert_callable(
        &mut self,
        kind: SymbolKindRef,
        name_node: Option<Node<'_>>,
        arg_types: Vec<Option<String>>,
        result_type: Option<String>,
    ) {
        let Some(name_node) = name_node else {
            return;
        };
        let Some(name) = node_text(self.source, name_node) else {
            return;
        };
        let decl_kind = match kind {
            SymbolKindRef::Function => SymbolKindDecl::Function,
            SymbolKindRef::Process => SymbolKindDecl::Process,
            SymbolKindRef::Event => SymbolKindDecl::Event,
            SymbolKindRef::Predicate => SymbolKindDecl::Predicate,
            SymbolKindRef::Table => SymbolKindDecl::Table,
        };
        if self.name_defined_elsewhere(&name, Some(decl_kind)) {
            self.diagnostics.push(Diagnostic {
                range: range_from_node(self.source, name_node),
                severity: Some(DiagnosticSeverity::WARNING),
                message: format!("identifier `{name}` already defined"),
                ..Diagnostic::default()
            });
        }
        let decl = CallableDecl {
            arity: arg_types.len(),
            arg_types,
            result_type,
            range: name_node.byte_range(),
        };

        let table = match kind {
            SymbolKindRef::Function => &mut self.globals.functions,
            SymbolKindRef::Process => &mut self.globals.processes,
            SymbolKindRef::Event => &mut self.globals.events,
            SymbolKindRef::Predicate => &mut self.globals.predicates,
            SymbolKindRef::Table => &mut self.globals.tables,
        };

        if let Some(existing) = table.get(&name) {
            if existing.arity != decl.arity {
                self.diagnostics.push(Diagnostic {
                    range: range_from_bytes(self.source, decl.range.clone()),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!(
                        "conflicting declaration for `{name}`: expected arity {}, found {}",
                        existing.arity, decl.arity
                    ),
                    ..Diagnostic::default()
                });
            }
            return;
        }

        table.insert(name, decl);
    }

    fn insert_extra_callable(
        &mut self,
        kind: SymbolKindDecl,
        name_node: Option<Node<'_>>,
        arg_types: Vec<Option<String>>,
        result_type: Option<String>,
    ) {
        let Some(name_node) = name_node else {
            return;
        };
        let Some(name) = node_text(self.source, name_node) else {
            return;
        };
        if self.name_defined_elsewhere(&name, Some(kind)) {
            self.diagnostics.push(Diagnostic {
                range: range_from_node(self.source, name_node),
                severity: Some(DiagnosticSeverity::WARNING),
                message: format!("identifier `{name}` already defined"),
                ..Diagnostic::default()
            });
        }
        let decl = CallableDecl {
            arity: arg_types.len(),
            arg_types,
            result_type,
            range: name_node.byte_range(),
        };
        let table = match kind {
            SymbolKindDecl::Predicate => &mut self.globals.predicates,
            SymbolKindDecl::Table => &mut self.globals.tables,
            _ => return,
        };
        table.insert(name, decl);
    }

    fn name_defined_elsewhere(&self, name: &str, current: Option<SymbolKindDecl>) -> bool {
        let checks = [
            (SymbolKindDecl::Type, self.globals.types.contains(name)),
            (SymbolKindDecl::Function, self.globals.functions.contains_key(name)),
            (SymbolKindDecl::Process, self.globals.processes.contains_key(name)),
            (SymbolKindDecl::Event, self.globals.events.contains_key(name)),
            (SymbolKindDecl::Predicate, self.globals.predicates.contains_key(name)),
            (SymbolKindDecl::Table, self.globals.tables.contains_key(name)),
            (SymbolKindDecl::Name, self.globals.names.contains_key(name)),
        ];
        checks
            .into_iter()
            .any(|(kind, exists)| exists && current != Some(kind))
    }

    fn check_top_level(&mut self, root: Node<'_>) {
        for node in root.named_children(&mut root.walk()) {
            match node.kind() {
                "let_decl" => {
                    let mut env = LocalEnv::new();
                    extend_with_binder_parameters(&mut env, node, self.source);
                    if let Some(body) = node.child_by_field_name("body") {
                        self.check_process(body, &env);
                    }
                }
                "letfun_decl" => {
                    let mut env = LocalEnv::new();
                    extend_with_binder_parameters(&mut env, node, self.source);
                    if let Some(body) = node.child_by_field_name("body") {
                        self.check_term(body, &env);
                    }
                }
                "process_decl" => {
                    if let Some(body) = node.child_by_field_name("body") {
                        self.check_process(body, &LocalEnv::new());
                    }
                }
                "query_decl" => {
                    let mut env = LocalEnv::new();
                    if let Some(bindings) = node.child_by_field_name("bindings") {
                        extend_with_bindings_from_children(&mut env, bindings, self.source);
                    }
                    if let Some(body) = node.child_by_field_name("body") {
                        self.check_special_query_forms(body, &env, false);
                    }
                    self.validate_options_for_owner(node);
                    if let Some(body) = node.child_by_field_name("body") {
                        if body.kind() == "query_sequence" {
                            for item in body.named_children(&mut body.walk()) {
                                self.check_query_expr(item, &env);
                                self.check_no_reduc_functions_in_query(item);
                            }
                        } else {
                            self.check_query_expr(body, &env);
                            self.check_no_reduc_functions_in_query(body);
                        }
                    }
                }
                "not_decl" => {
                    let mut env = LocalEnv::new();
                    if let Some(bindings) = node.child_by_field_name("bindings") {
                        extend_with_bindings_from_children(&mut env, bindings, self.source);
                    }
                    if let Some(body) = node.child_by_field_name("body") {
                        self.check_query_expr(body, &env);
                        self.check_no_reduc_functions_in_query(body);
                    }
                }
                "noninterf_decl" => {
                    let mut env = LocalEnv::new();
                    if let Some(bindings) = node.child_by_field_name("bindings") {
                        extend_with_bindings_from_children(&mut env, bindings, self.source);
                    }
                    if let Some(body) = node.child_by_field_name("body") {
                        self.check_noninterf_body(body, &env);
                    }
                }
                "weaksecret_decl" => {
                    if let Some(body) = node.child_by_field_name("body") {
                        for ident in body.named_children(&mut body.walk()) {
                            if ident.kind() == "identifier" {
                                self.check_private_free_name(
                                    ident,
                                    "weaksecret can only be tested on private free names",
                                );
                            }
                        }
                    }
                }
                "lemma_decl" => {
                    let mut env = LocalEnv::new();
                    if let Some(bindings) = node.child_by_field_name("bindings") {
                        extend_with_bindings_from_children(&mut env, bindings, self.source);
                    }
                    if let Some(body) = node.child_by_field_name("body") {
                        self.check_special_query_forms(body, &env, true);
                        if body.kind() == "query_sequence" {
                            for item in body.named_children(&mut body.walk()) {
                                self.check_query_expr(item, &env);
                                self.check_no_reduc_functions_in_query(item);
                            }
                        } else {
                            self.check_query_expr(body, &env);
                            self.check_no_reduc_functions_in_query(body);
                        }
                    }
                }
                "reduc_decl" | "equation_decl" => {
                    self.check_rule_container(node);
                }
                _ => {}
            }
        }
    }

    fn check_rule_container(&mut self, node: Node<'_>) {
        let mut expected_top_function: Option<(String, Vec<Option<String>>)> = None;
        for clause in node.named_children(&mut node.walk()) {
            if clause.kind() != "rule_clause" {
                continue;
            }
            let mut env = LocalEnv::new();
            for child in clause.named_children(&mut clause.walk()) {
                if child.kind() == "forall_clause" {
                    extend_with_bindings_from_children(&mut env, child, self.source);
                }
            }
            if let Some(body) = clause.child_by_field_name("body") {
                self.check_term(body, &env);
                self.check_rule_clause(node.kind(), body, &env, &mut expected_top_function);
            }
        }
    }

    fn check_rule_clause(
        &mut self,
        container_kind: &str,
        body: Node<'_>,
        env: &LocalEnv,
        expected_top_function: &mut Option<(String, Vec<Option<String>>)>,
    ) {
        let Some(binary) = first_named_descendant_of_kind(body, "binary_expr") else {
            self.diagnostics.push(Diagnostic {
                range: range_from_node(self.source, body),
                severity: Some(DiagnosticSeverity::WARNING),
                message: format!("`{container_kind}` rule should be an equality"),
                ..Diagnostic::default()
            });
            return;
        };

        if operator_text(self.source, binary).as_deref() != Some("=") {
            self.diagnostics.push(Diagnostic {
                range: range_from_node(self.source, binary),
                severity: Some(DiagnosticSeverity::WARNING),
                message: format!("`{container_kind}` rule should use `=`"),
                ..Diagnostic::default()
            });
        }

        let Some(left) = binary.child_by_field_name("left") else {
            return;
        };
        let Some(right) = binary.child_by_field_name("right") else {
            return;
        };

        if let (Some(left_ty), Some(right_ty)) = (
            self.infer_expr_type(left, env),
            self.infer_expr_type(right, env),
        ) {
            if left_ty != right_ty {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, binary),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!(
                        "`{container_kind}` rule has incompatible sides `{left_ty}` and `{right_ty}`"
                    ),
                    ..Diagnostic::default()
                });
            }
        }

        if container_kind != "reduc_decl" {
            return;
        }

        if left.kind() != "call" {
            self.diagnostics.push(Diagnostic {
                range: range_from_node(self.source, left),
                severity: Some(DiagnosticSeverity::WARNING),
                message: "\"reduc\" rule should begin with a function application".into(),
                ..Diagnostic::default()
            });
            return;
        }

        let Some(function) = left.child_by_field_name("function") else {
            return;
        };
        let Some(name) = node_text(self.source, function) else {
            return;
        };
        let arg_types = left
            .child_by_field_name("arguments")
            .map(|args| {
                collect_argument_nodes(args)
                    .into_iter()
                    .map(|arg| self.infer_expr_type(arg, env))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if let Some((expected_name, expected_types)) = expected_top_function.as_ref() {
            if expected_name != &name {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, function),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!(
                        "\"reduc\" rules should all begin with the same function `{expected_name}`"
                    ),
                    ..Diagnostic::default()
                });
            } else if expected_types.len() == arg_types.len()
                && expected_types
                    .iter()
                    .zip(arg_types.iter())
                    .any(|(left, right)| left != right)
            {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, function),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!(
                        "\"reduc\" rules for `{name}` should use the same argument types"
                    ),
                    ..Diagnostic::default()
                });
            }
        } else {
            *expected_top_function = Some((name, arg_types));
        }
    }

    fn check_process(&mut self, node: Node<'_>, env: &LocalEnv) {
        match node.kind() {
            "sequenced_process" => {
                let Some(left) = node.child_by_field_name("left") else {
                    return;
                };
                let Some(right) = node.child_by_field_name("right") else {
                    return;
                };
                self.check_process(left, env);
                let right_env = self.env_after_process(left, env);
                self.check_process(right, &right_env);
            }
            "parallel_process" => {
                if let Some(left) = node.child_by_field_name("left") {
                    self.check_process(left, env);
                }
                if let Some(right) = node.child_by_field_name("right") {
                    self.check_process(right, env);
                }
            }
            "grouped_process" | "replicated_process" => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_process(child, env);
                }
            }
            "new_process" => {
                if let Some(binding) = node.child_by_field_name("binding") {
                    let mut inner = env.clone();
                    extend_with_pattern_binding_like(&mut inner, binding, self.source);
                    if let Some(body) = node.child_by_field_name("body") {
                        self.check_process(body, &inner);
                    }
                }
            }
            "in_process" => {
                if let Some(channel) = node.child_by_field_name("channel") {
                    self.check_term(channel, env);
                    self.check_expected_type(channel, env, "channel", "this term should have type channel");
                }
                self.validate_options_for_owner(node);
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    self.check_pattern_terms(pattern, env);
                }
            }
            "out_process" => {
                let mut walk = node.walk();
                let mut iter = node.named_children(&mut walk);
                if let Some(channel) = iter.next() {
                    self.check_term(channel, env);
                    self.check_expected_type(channel, env, "channel", "this term should have type channel");
                }
                for child in iter {
                    self.check_term(child, env);
                }
            }
            "insert_process" => {
                let Some(table_node) = node.child_by_field_name("table") else {
                    return;
                };
                let Some(name) = node_text(self.source, table_node) else {
                    return;
                };
                let arguments = node
                    .child_by_field_name("arguments")
                    .map(collect_argument_nodes)
                    .unwrap_or_default();
                for argument in &arguments {
                    self.check_term(*argument, env);
                }
                self.check_callable_reference(
                    SymbolKindRef::Table,
                    &name,
                    arguments.len(),
                    table_node.byte_range(),
                    arguments
                        .iter()
                        .map(|argument| self.infer_expr_type(*argument, env))
                        .collect(),
                );
            }
            "get_process" => {
                let Some(table_node) = node.child_by_field_name("table") else {
                    return;
                };
                let Some(name) = node_text(self.source, table_node) else {
                    return;
                };
                let patterns = node
                    .child_by_field_name("patterns")
                    .map(collect_pattern_nodes)
                    .unwrap_or_default();
                self.validate_options_for_owner(node);
                for pattern in &patterns {
                    self.check_pattern_terms(*pattern, env);
                }
                self.check_callable_reference(
                    SymbolKindRef::Table,
                    &name,
                    patterns.len(),
                    table_node.byte_range(),
                    vec![None; patterns.len()],
                );
                if let Some(condition_clause) = first_named_child_of_kind(node, "suchthat_clause") {
                    if let Some(condition) = condition_clause.child_by_field_name("condition") {
                        self.check_term(condition, env);
                        if let Some(ty) = self.infer_expr_type(condition, env) {
                            if ty != "bool" {
                                self.diagnostics.push(Diagnostic {
                                    range: range_from_node(self.source, condition),
                                    severity: Some(DiagnosticSeverity::WARNING),
                                    message: format!(
                                        "get condition has type `{ty}`, expected `bool`"
                                    ),
                                    ..Diagnostic::default()
                                });
                            }
                        }
                        self.check_get_condition_restrictions(condition);
                    }
                }
                let mut body_env = env.clone();
                for pattern in patterns {
                    extend_with_pattern(&mut body_env, pattern, self.source);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.check_process(body, &body_env);
                }
                if let Some(alternative) = node.child_by_field_name("alternative") {
                    self.check_process(alternative, env);
                }
            }
            "let_process" => {
                if let Some(value) = node.child_by_field_name("value") {
                    self.check_term(value, env);
                }
                let mut body_env = env.clone();
                if let Some(binding) = node.child_by_field_name("binding") {
                    self.check_pattern_terms(binding, env);
                    extend_with_pattern(&mut body_env, binding, self.source);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.check_process(body, &body_env);
                }
                if let Some(alternative) = node.child_by_field_name("alternative") {
                    self.check_process(alternative, env);
                }
            }
            "if_process" => {
                if let Some(condition) = node.child_by_field_name("condition") {
                    self.check_query_expr(condition, env);
                    if let Some(ty) = self.infer_expr_type(condition, env) {
                        if ty != "bool" {
                            self.diagnostics.push(Diagnostic {
                                range: range_from_node(self.source, condition),
                                severity: Some(DiagnosticSeverity::WARNING),
                                message: format!("if condition has type `{ty}`, expected `bool`"),
                                ..Diagnostic::default()
                            });
                        }
                    }
                }
                if let Some(consequence) = node.child_by_field_name("consequence") {
                    self.check_process(consequence, env);
                }
                if let Some(alternative) = node.child_by_field_name("alternative") {
                    self.check_process(alternative, env);
                }
            }
            "event_process" => {
                if let Some(value) = node.child_by_field_name("value") {
                    self.check_event_reference(value, env);
                }
            }
            "call_process" => {
                self.check_process_call(node, env);
            }
            "phase_process" => {}
            "nil_process" => {}
            _ => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_process(child, env);
                }
            }
        }
    }

    fn check_query_expr(&mut self, node: Node<'_>, env: &LocalEnv) {
        match node.kind() {
            "grouped_query" => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_query_expr(child, env);
                }
            }
            "query_expr" => {
                for child in node.named_children(&mut node.walk()) {
                    match child.kind() {
                        "prefix_query" => self.check_prefix_query(child, env),
                        _ => self.check_query_expr(child, env),
                    }
                }
            }
            "binary_expr" | "unary_expr" => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_query_expr(child, env);
                }
                match node.kind() {
                    "binary_expr" => self.check_binary_expr(node, env),
                    "unary_expr" => self.check_unary_expr(node, env),
                    _ => {}
                }
            }
            "prefix_query" => self.check_prefix_query(node, env),
            _ => self.check_term(node, env),
        }
    }

    fn check_noninterf_body(&mut self, node: Node<'_>, env: &LocalEnv) {
        match node.kind() {
            "noninterf_sequence" | "term_sequence" => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_noninterf_body(child, env);
                }
            }
            "noninterf_item" => {
                let mut expected_type = None;
                if let Some(name) = node.child_by_field_name("name") {
                    expected_type = self.check_private_free_name(
                        name,
                        "noninterf can only be tested on private free names",
                    );
                }
                if let Some(values) = node.child_by_field_name("values") {
                    for child in values.named_children(&mut values.walk()) {
                        self.check_term(child, env);
                        if let (Some(expected), Some(actual)) = (
                            expected_type.as_deref(),
                            self.infer_expr_type(child, env).as_deref(),
                        ) {
                            if expected != actual {
                                self.diagnostics.push(Diagnostic {
                                    range: range_from_node(self.source, child),
                                    severity: Some(DiagnosticSeverity::WARNING),
                                    message: format!(
                                        "noninterf value has type `{actual}`, expected `{expected}`"
                                    ),
                                    ..Diagnostic::default()
                                });
                            }
                        }
                    }
                }
            }
            _ => self.check_term(node, env),
        }
    }

    fn check_private_free_name(&mut self, node: Node<'_>, message: &str) -> Option<String> {
        let Some(name) = node_text(self.source, node) else {
            return None;
        };
        match self.globals.names.get(&name) {
            Some(value) if value.is_free && value.is_private => value.type_name.clone(),
            Some(_) => {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: message.into(),
                    ..Diagnostic::default()
                });
                None
            }
            None => {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: unknown_symbol_message(
                        "name",
                        &name,
                        &self.globals.names.keys().cloned().collect::<Vec<_>>(),
                    ),
                    ..Diagnostic::default()
                });
                None
            }
        }
    }

    fn check_special_query_forms(&mut self, node: Node<'_>, env: &LocalEnv, in_lemma: bool) {
        match node.kind() {
            "query_sequence" | "query_expr" | "grouped_query" | "binary_expr" | "unary_expr" => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_special_query_forms(child, env, in_lemma);
                }
            }
            "prefix_query" => self.check_special_prefix_query(node, env, in_lemma),
            _ => {}
        }
    }

    fn check_special_prefix_query(&mut self, node: Node<'_>, env: &LocalEnv, in_lemma: bool) {
        let Some(prefix) = leading_keyword(self.source, node) else {
            return;
        };
        match prefix.as_str() {
            "secret" | "public_vars" => {
                if let Some(list) = first_named_child_of_kind(node, "identifier_list") {
                    for ident in list.named_children(&mut list.walk()) {
                        if ident.kind() == "identifier" {
                            self.check_bound_name_or_variable(ident, env, prefix.as_str());
                        }
                    }
                }
                if in_lemma && prefix == "secret" {
                    self.diagnostics.push(Diagnostic {
                        range: range_from_node(self.source, node),
                        severity: Some(DiagnosticSeverity::WARNING),
                        message: "lemmas, axioms, and restrictions should be correspondence queries".into(),
                        ..Diagnostic::default()
                    });
                }
            }
            "putbegin" => {
                if in_lemma {
                    self.diagnostics.push(Diagnostic {
                        range: range_from_node(self.source, node),
                        severity: Some(DiagnosticSeverity::WARNING),
                        message: "lemmas, axioms, and restrictions should be correspondence queries".into(),
                        ..Diagnostic::default()
                    });
                }
                let text = snippet_for_node(self.source, node);
                if (text.starts_with("putbegin event:") || text.starts_with("putbegin inj-event:"))
                    && first_named_child_of_kind(node, "identifier_list").is_some()
                {
                    let list = first_named_child_of_kind(node, "identifier_list").unwrap();
                    for ident in list.named_children(&mut list.walk()) {
                        if ident.kind() != "identifier" {
                            continue;
                        }
                        let Some(name) = node_text(self.source, ident) else {
                            continue;
                        };
                        if !self.globals.events.contains_key(&name) {
                            self.diagnostics.push(Diagnostic {
                                range: range_from_node(self.source, ident),
                                severity: Some(DiagnosticSeverity::WARNING),
                                message: unknown_symbol_message(
                                    "event",
                                    &name,
                                    &self.globals.events.keys().cloned().collect::<Vec<_>>(),
                                ),
                                ..Diagnostic::default()
                            });
                        }
                    }
                }
            }
            "phase" => {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: "phase can only be used with attacker, mess, or table".into(),
                    ..Diagnostic::default()
                });
            }
            _ => {}
        }
    }

    fn check_bound_name_or_variable(&mut self, node: Node<'_>, env: &LocalEnv, context: &str) {
        let Some(name) = node_text(self.source, node) else {
            return;
        };
        match self.lookup_identifier(env, &name) {
            IdentifierLookup::Variable(_) | IdentifierLookup::Name(_) => {}
            IdentifierLookup::Function { .. } | IdentifierLookup::Unknown => {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!("`{context}` expects bound names or variables; `{name}` is not one"),
                    ..Diagnostic::default()
                });
            }
            IdentifierLookup::ZeroArgFunction(_) | IdentifierLookup::ZeroArgPredicate => {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!("`{context}` expects bound names or variables; `{name}` is not one"),
                    ..Diagnostic::default()
                });
            }
        }
    }

    fn validate_options_for_owner(&mut self, owner: Node<'_>) {
        let Some(options) = first_named_child_of_kind(owner, "options") else {
            return;
        };
        let Some(policy) = option_policy_for_owner(owner, self.source) else {
            return;
        };
        self.validate_allowed_options(options, policy);
    }

    fn validate_allowed_options(&mut self, options: Node<'_>, policy: OptionPolicy) {
        for option in options.named_children(&mut options.walk()) {
            if option.kind() != "identifier" {
                continue;
            }
            let Some(name) = node_text(self.source, option) else {
                continue;
            };
            if !policy.allows(&name) {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, option),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: policy.error_message().to_owned(),
                    ..Diagnostic::default()
                });
            }
        }
    }

    fn check_get_condition_restrictions(&mut self, node: Node<'_>) {
        if node.kind() == "new_name" {
            self.diagnostics.push(Diagnostic {
                range: range_from_node(self.source, node),
                severity: Some(DiagnosticSeverity::WARNING),
                message: "`new` is not allowed in conditions of `get`".into(),
                ..Diagnostic::default()
            });
        }
        for child in node.named_children(&mut node.walk()) {
            self.check_get_condition_restrictions(child);
        }
    }

    fn env_after_process(&self, node: Node<'_>, env: &LocalEnv) -> LocalEnv {
        match node.kind() {
            "sequenced_process" => {
                let Some(left) = node.child_by_field_name("left") else {
                    return env.clone();
                };
                let Some(right) = node.child_by_field_name("right") else {
                    return env.clone();
                };
                let mid = self.env_after_process(left, env);
                self.env_after_process(right, &mid)
            }
            "new_process" => {
                let mut next = env.clone();
                if let Some(binding) = node.child_by_field_name("binding") {
                    extend_with_binding(&mut next, binding, self.source);
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.env_after_process(body, &next)
                } else {
                    next
                }
            }
            "grouped_process" => {
                let Some(body) = node
                    .named_children(&mut node.walk())
                    .find(|child| child.kind() != "comment")
                else {
                    return env.clone();
                };
                self.env_after_process(body, env)
            }
            "replicated_process" => {
                let Some(body) = node.child_by_field_name("body") else {
                    return env.clone();
                };
                self.env_after_process(body, env)
            }
            "in_process" => {
                let mut next = env.clone();
                if let Some(pattern) = node.child_by_field_name("pattern") {
                    extend_with_pattern(&mut next, pattern, self.source);
                }
                next
            }
            "get_process" => {
                let mut next = env.clone();
                if let Some(patterns) = node.child_by_field_name("patterns") {
                    for pattern in collect_pattern_nodes(patterns) {
                        extend_with_pattern(&mut next, pattern, self.source);
                    }
                }
                if let Some(body) = node.child_by_field_name("body") {
                    self.env_after_process(body, &next)
                } else {
                    next
                }
            }
            _ => env.clone(),
        }
    }

    fn check_prefix_query(&mut self, node: Node<'_>, env: &LocalEnv) {
        let Some(prefix) = leading_keyword(self.source, node) else {
            for child in node.named_children(&mut node.walk()) {
                match child.kind() {
                    "event_call" => self.check_event_reference(child, env),
                    _ => self.check_term(child, env),
                }
            }
            return;
        };

        match prefix.as_str() {
            "event" | "inj-event" => {
                for child in node.named_children(&mut node.walk()) {
                    match child.kind() {
                        "event_call" => self.check_event_reference(child, env),
                        _ => self.check_term(child, env),
                    }
                }
            }
            "attacker" => {
                if let Some(payload) = first_named_child_of_kind(node, "call_like_payload") {
                    let arguments = collect_argument_nodes(payload);
                    for argument in &arguments {
                        self.check_term(*argument, env);
                    }
                    if arguments.len() != 1 {
                        self.diagnostics.push(Diagnostic {
                            range: range_from_node(self.source, payload),
                            severity: Some(DiagnosticSeverity::WARNING),
                            message: "predicate `attacker` expects 1 argument".into(),
                            ..Diagnostic::default()
                        });
                    }
                }
            }
            "mess" => {
                if let Some(payload) = first_named_child_of_kind(node, "call_like_payload") {
                    let arguments = collect_argument_nodes(payload);
                    for argument in &arguments {
                        self.check_term(*argument, env);
                    }
                    if arguments.len() != 2 {
                        self.diagnostics.push(Diagnostic {
                            range: range_from_node(self.source, payload),
                            severity: Some(DiagnosticSeverity::WARNING),
                            message: "predicate `mess` expects 2 arguments".into(),
                            ..Diagnostic::default()
                        });
                    } else if let Some(channel) = arguments.first() {
                        self.check_expected_type(
                            *channel,
                            env,
                            "channel",
                            "first argument of `mess` should have type channel",
                        );
                    }
                }
            }
            "table" => {
                if let Some(payload) = first_named_child_of_kind(node, "call_like_payload") {
                    let arguments = collect_argument_nodes(payload);
                    if arguments.len() != 1 {
                        self.diagnostics.push(Diagnostic {
                            range: range_from_node(self.source, payload),
                            severity: Some(DiagnosticSeverity::WARNING),
                            message: "predicate `table` expects 1 argument".into(),
                            ..Diagnostic::default()
                        });
                    } else if let Some(table_term) = arguments.first() {
                        self.check_table_term(*table_term, env);
                    }
                }
            }
            "public_vars" | "putbegin" | "secret" => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_term(child, env);
                }
            }
            "phase" => {}
            _ => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_term(child, env);
                }
            }
        }
    }

    fn check_no_reduc_functions_in_query(&mut self, node: Node<'_>) {
        if node.kind() == "call" {
            if let Some(function) = node.child_by_field_name("function") {
                if let Some(name) = node_text(self.source, function) {
                    if self.globals.reduc_functions.contains(&name) {
                        self.diagnostics.push(Diagnostic {
                            range: range_from_node(self.source, function),
                            severity: Some(DiagnosticSeverity::WARNING),
                            message: format!(
                                "function `{name}` is defined by `reduc`; it should not appear in a query"
                            ),
                            ..Diagnostic::default()
                        });
                    }
                }
            }
        }
        for child in node.named_children(&mut node.walk()) {
            self.check_no_reduc_functions_in_query(child);
        }
    }

    fn check_event_reference(&mut self, node: Node<'_>, env: &LocalEnv) {
        match node.kind() {
            "identifier" => {
                let Some(name) = node_text(self.source, node) else {
                    return;
                };
                self.check_callable_reference(
                    SymbolKindRef::Event,
                    &name,
                    0,
                    node.byte_range(),
                    Vec::new(),
                );
            }
            "event_call" => {
                let mut walk = node.walk();
                let mut children = node.named_children(&mut walk);
                let Some(first) = children.next() else {
                    return;
                };
                match first.kind() {
                    "identifier" => {
                        let Some(name) = node_text(self.source, first) else {
                            return;
                        };
                        self.check_callable_reference(
                            SymbolKindRef::Event,
                            &name,
                            0,
                            first.byte_range(),
                            Vec::new(),
                        );
                    }
                    "call" => {
                        let Some(function) = first.child_by_field_name("function") else {
                            return;
                        };
                        let Some(name) = node_text(self.source, function) else {
                            return;
                        };
                        let arguments = first
                            .child_by_field_name("arguments")
                            .map(|args| collect_argument_nodes(args))
                            .unwrap_or_default();
                        for arg in &arguments {
                            self.check_term(*arg, env);
                        }
                        self.check_callable_reference(
                            SymbolKindRef::Event,
                            &name,
                            arguments.len(),
                            function.byte_range(),
                            arguments
                                .iter()
                                .map(|arg| self.infer_expr_type(*arg, env))
                                .collect(),
                        );
                    }
                    _ => self.check_term(first, env),
                }
            }
            "call" => {
                let Some(function) = node.child_by_field_name("function") else {
                    return;
                };
                let Some(name) = node_text(self.source, function) else {
                    return;
                };
                let arguments = node
                    .child_by_field_name("arguments")
                    .map(collect_argument_nodes)
                    .unwrap_or_default();
                for arg in &arguments {
                    self.check_term(*arg, env);
                }
                self.check_callable_reference(
                    SymbolKindRef::Event,
                    &name,
                    arguments.len(),
                    function.byte_range(),
                    arguments
                        .iter()
                        .map(|arg| self.infer_expr_type(*arg, env))
                        .collect(),
                );
            }
            _ => self.check_term(node, env),
        }
    }

    fn check_table_term(&mut self, node: Node<'_>, env: &LocalEnv) {
        match node.kind() {
            "call" => {
                let Some(function) = node.child_by_field_name("function") else {
                    return;
                };
                let Some(name) = node_text(self.source, function) else {
                    return;
                };
                let arguments = node
                    .child_by_field_name("arguments")
                    .map(collect_argument_nodes)
                    .unwrap_or_default();
                for argument in &arguments {
                    self.check_term(*argument, env);
                }
                self.check_callable_reference(
                    SymbolKindRef::Table,
                    &name,
                    arguments.len(),
                    function.byte_range(),
                    arguments
                        .iter()
                        .map(|argument| self.infer_expr_type(*argument, env))
                        .collect(),
                );
            }
            _ => {
                self.check_term(node, env);
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: "`table` expects a table application".into(),
                    ..Diagnostic::default()
                });
            }
        }
    }

    fn check_term(&mut self, node: Node<'_>, env: &LocalEnv) {
        match node.kind() {
            "identifier" => self.check_identifier_reference(node, env),
            "binding" => self.check_binding_reference(node, env),
            "call" => self.check_function_call(node, env),
            "new_name" => {
                for child in node.named_children(&mut node.walk()) {
                    if child.kind() == "name_binding" {
                        if let Some(value) = child.child_by_field_name("value") {
                            self.check_term(value, env);
                        }
                    }
                }
            }
            "new_term" => {
                if let Some(binding) = node.child_by_field_name("binding") {
                    let mut inner = env.clone();
                    extend_with_pattern_binding_like(&mut inner, binding, self.source);
                    if let Some(body) = node.child_by_field_name("body") {
                        self.check_term(body, &inner);
                    }
                }
            }
            "if_term" => {
                if let Some(condition) = node.child_by_field_name("condition") {
                    self.check_query_expr(condition, env);
                    if let Some(ty) = self.infer_expr_type(condition, env) {
                        if ty != "bool" {
                            self.diagnostics.push(Diagnostic {
                                range: range_from_node(self.source, condition),
                                severity: Some(DiagnosticSeverity::WARNING),
                                message: format!("if condition has type `{ty}`, expected `bool`"),
                                ..Diagnostic::default()
                            });
                        }
                    }
                }
                if let Some(consequence) = node.child_by_field_name("consequence") {
                    self.check_term(consequence, env);
                }
                if let Some(alternative) = node.child_by_field_name("alternative") {
                    self.check_term(alternative, env);
                }
            }
            "choice_term" => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_term(child, env);
                }
                self.check_choice_term(node, env);
            }
            "binary_expr" => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_term(child, env);
                }
                self.check_binary_expr(node, env);
            }
            "unary_expr" => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_term(child, env);
                }
                self.check_unary_expr(node, env);
            }
            "tuple" | "grouped_term" => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_term(child, env);
                }
            }
            "query_expr" => self.check_query_expr(node, env),
            "number" | "string" | "boolean" | "parameter" | "fail_term" => {}
            _ => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_term(child, env);
                }
            }
        }
    }

    fn check_pattern_terms(&mut self, node: Node<'_>, env: &LocalEnv) {
        match node.kind() {
            "pattern" | "grouped_pattern" | "tuple_pattern" | "call_pattern" | "pattern_arguments" => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_pattern_terms(child, env);
                }
            }
            "equality_pattern" => {
                for child in node.named_children(&mut node.walk()) {
                    self.check_term(child, env);
                }
            }
            _ => {}
        }
    }

    fn check_identifier_reference(&mut self, node: Node<'_>, env: &LocalEnv) {
        let Some(parent) = node.parent() else {
            return;
        };
        if is_non_reference_identifier(node, parent) {
            return;
        }

        let Some(name) = node_text(self.source, node) else {
            return;
        };

        match self.lookup_identifier(env, &name) {
            IdentifierLookup::Variable(_)
            | IdentifierLookup::Name(_)
            | IdentifierLookup::ZeroArgFunction(_)
            | IdentifierLookup::ZeroArgPredicate => {}
            IdentifierLookup::Function { arity } => {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!(
                        "function `{name}` expects {} argument(s) but is used without arguments",
                        arity
                    ),
                    ..Diagnostic::default()
                });
            }
            IdentifierLookup::Unknown => {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: unknown_symbol_message(
                        "identifier",
                        &name,
                        &identifier_candidates(env, &self.globals),
                    ),
                    ..Diagnostic::default()
                });
            }
        }
    }

    fn check_binding_reference(&mut self, node: Node<'_>, env: &LocalEnv) {
        if node.child_by_field_name("type").is_some() {
            return;
        }
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        if name_node.kind() != "identifier" {
            return;
        }
        let Some(name) = node_text(self.source, name_node) else {
            return;
        };
        match self.lookup_identifier(env, &name) {
            IdentifierLookup::Variable(_)
            | IdentifierLookup::Name(_)
            | IdentifierLookup::ZeroArgFunction(_)
            | IdentifierLookup::ZeroArgPredicate => {}
            IdentifierLookup::Function { arity } => {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, name_node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!(
                        "function `{name}` expects {} argument(s) but is used without arguments",
                        arity
                    ),
                    ..Diagnostic::default()
                });
            }
            IdentifierLookup::Unknown => {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, name_node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: unknown_symbol_message(
                        "variable",
                        &name,
                        &identifier_candidates(env, &self.globals),
                    ),
                    ..Diagnostic::default()
                });
            }
        }
    }

    fn check_function_call(&mut self, node: Node<'_>, env: &LocalEnv) {
        let Some(function) = node.child_by_field_name("function") else {
            return;
        };
        let Some(name) = node_text(self.source, function) else {
            return;
        };
        let arguments = node
            .child_by_field_name("arguments")
            .map(collect_argument_nodes)
            .unwrap_or_default();
        for arg in &arguments {
            self.check_term(*arg, env);
        }
        let arg_types = arguments
            .iter()
            .map(|arg| self.infer_expr_type(*arg, env))
            .collect();
        let kind = if self.globals.functions.contains_key(&name) {
            SymbolKindRef::Function
        } else if self.globals.predicates.contains_key(&name) {
            SymbolKindRef::Predicate
        } else {
            SymbolKindRef::Function
        };
        self.check_callable_reference(kind, &name, arguments.len(), function.byte_range(), arg_types);
    }

    fn check_process_call(&mut self, node: Node<'_>, env: &LocalEnv) {
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        let Some(name) = node_text(self.source, name_node) else {
            return;
        };
        let arguments = first_named_child_of_kind(node, "arguments")
            .map(collect_argument_nodes)
            .unwrap_or_default();
        for arg in &arguments {
            self.check_term(*arg, env);
        }
        let arg_types = arguments
            .iter()
            .map(|arg| self.infer_expr_type(*arg, env))
            .collect();
        self.check_callable_reference(
            SymbolKindRef::Process,
            &name,
            arguments.len(),
            name_node.byte_range(),
            arg_types,
        );
    }

    fn check_callable_reference(
        &mut self,
        kind: SymbolKindRef,
        name: &str,
        arity: usize,
        range: Range<usize>,
        actual_types: Vec<Option<String>>,
    ) {
        let decl = self.lookup_callable(kind, name).cloned();
        let candidates = self.callable_candidates(kind);
        let label = self.callable_label(kind);

        let Some(decl) = decl else {
            self.diagnostics.push(Diagnostic {
                range: range_from_bytes(self.source, range.clone()),
                severity: Some(DiagnosticSeverity::WARNING),
                message: unknown_symbol_message(label, name, &candidates),
                ..Diagnostic::default()
            });
            return;
        };

        if decl.arity != arity {
            self.diagnostics.push(Diagnostic {
                range: range_from_bytes(self.source, range.clone()),
                severity: Some(DiagnosticSeverity::WARNING),
                message: format!(
                    "{label} `{name}` expects {} argument(s) but is given {arity}",
                    decl.arity
                ),
                ..Diagnostic::default()
            });
        }

        for (expected, actual) in decl.arg_types.iter().zip(actual_types.iter()) {
            if let (Some(expected), Some(actual)) = (expected.as_deref(), actual.as_deref()) {
                if expected != actual {
                    self.diagnostics.push(Diagnostic {
                        range: range_from_bytes(self.source, range.clone()),
                        severity: Some(DiagnosticSeverity::WARNING),
                        message: format!(
                            "{label} `{name}` expects argument of type `{expected}` but got `{actual}`"
                        ),
                        ..Diagnostic::default()
                    });
                    break;
                }
            }
        }
    }

    fn check_expected_type(&mut self, node: Node<'_>, env: &LocalEnv, expected: &str, message: &str) {
        if let Some(actual) = self.infer_expr_type(node, env) {
            if actual != expected {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!("{message}; got `{actual}`"),
                    ..Diagnostic::default()
                });
            }
        }
    }

    fn check_binary_expr(&mut self, node: Node<'_>, env: &LocalEnv) {
        let Some(op) = operator_text(self.source, node) else {
            return;
        };
        let Some(left) = node.child_by_field_name("left") else {
            return;
        };
        let Some(right) = node.child_by_field_name("right") else {
            return;
        };
        let left_ty = self.infer_expr_type(left, env);
        let right_ty = self.infer_expr_type(right, env);

        match op.as_str() {
            "&&" | "||" => {
                self.expect_expr_type(left, left_ty.as_deref(), "bool", "logical operator expects bool operands");
                self.expect_expr_type(right, right_ty.as_deref(), "bool", "logical operator expects bool operands");
            }
            "<" | "<=" | ">" | ">=" => {
                self.expect_expr_type(left, left_ty.as_deref(), "nat", "comparison expects nat operands");
                self.expect_expr_type(right, right_ty.as_deref(), "nat", "comparison expects nat operands");
            }
            "+" | "-" | "*" | "/" => {
                self.expect_expr_type(left, left_ty.as_deref(), "nat", "arithmetic operator expects nat operands");
                self.expect_expr_type(right, right_ty.as_deref(), "nat", "arithmetic operator expects nat operands");
            }
            "=" | "<>" => {
                if let (Some(left_ty), Some(right_ty)) = (left_ty.as_deref(), right_ty.as_deref()) {
                    if left_ty != right_ty {
                        self.diagnostics.push(Diagnostic {
                            range: range_from_node(self.source, node),
                            severity: Some(DiagnosticSeverity::WARNING),
                            message: format!(
                                "comparison uses incompatible types `{left_ty}` and `{right_ty}`"
                            ),
                            ..Diagnostic::default()
                        });
                    }
                }
            }
            _ => {}
        }
    }

    fn check_unary_expr(&mut self, node: Node<'_>, env: &LocalEnv) {
        let Some(op) = operator_text(self.source, node) else {
            return;
        };
        let mut walk = node.walk();
        let child = node.named_children(&mut walk).next();
        match (op.as_str(), child) {
            ("not", Some(expr)) => {
                let ty = self.infer_expr_type(expr, env);
                self.expect_expr_type(expr, ty.as_deref(), "bool", "`not` expects a bool operand");
            }
            _ => {}
        }
    }

    fn check_choice_term(&mut self, node: Node<'_>, env: &LocalEnv) {
        let Some(left) = node.child_by_field_name("left") else {
            return;
        };
        let Some(right) = node.child_by_field_name("right") else {
            return;
        };
        let left_ty = self.infer_expr_type(left, env);
        let right_ty = self.infer_expr_type(right, env);
        if let (Some(left_ty), Some(right_ty)) = (left_ty.as_deref(), right_ty.as_deref()) {
            if left_ty != right_ty {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!(
                        "`choice` expects two arguments of the same type, got `{left_ty}` and `{right_ty}`"
                    ),
                    ..Diagnostic::default()
                });
            }
        }
    }

    fn expect_expr_type(
        &mut self,
        node: Node<'_>,
        actual: Option<&str>,
        expected: &str,
        message: &str,
    ) {
        if let Some(actual) = actual {
            if actual != expected {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!("{message}; got `{actual}`"),
                    ..Diagnostic::default()
                });
            }
        }
    }

    fn validate_binder_parameters(&mut self, node: Node<'_>) {
        if let Some(parameters) = first_named_child_of_kind(node, "binder_parameters") {
            self.validate_type_nodes(parameters);
        }
    }

    fn validate_fun_decl(&mut self, node: Node<'_>) {
        let Some(name_node) = node.child_by_field_name("name") else {
            return;
        };
        let Some(name) = node_text(self.source, name_node) else {
            return;
        };
        let arity = node
            .child_by_field_name("parameters")
            .map(|p| count_type_expr_children(p))
            .unwrap_or(0);
        let Some(result) = node.child_by_field_name("result") else {
            return;
        };
        let result_ty = type_text(self.source, result);
        if arity == 0 && (result_ty == "nat" || result_ty == "bool") {
            self.diagnostics.push(Diagnostic {
                range: range_from_node(self.source, name_node),
                severity: Some(DiagnosticSeverity::WARNING),
                message: format!(
                    "constant `{name}` should not be declared with type `{result_ty}`"
                ),
                ..Diagnostic::default()
            });
        } else if arity > 0 && (result_ty == "nat" || result_ty == "bool") {
            self.diagnostics.push(Diagnostic {
                range: range_from_node(self.source, name_node),
                severity: Some(DiagnosticSeverity::WARNING),
                message: format!(
                    "function `{name}` should not use `{result_ty}` as a return type"
                ),
                ..Diagnostic::default()
            });
        }
    }

    fn validate_name_decl(&mut self, node: Node<'_>) {
        let Some(ty_node) = node.child_by_field_name("type") else {
            return;
        };
        let ty = type_text(self.source, ty_node);
        if ty != "nat" && ty != "bool" {
            return;
        }
        let Some(names) = node.child_by_field_name("name") else {
            return;
        };
        for ident in names.named_children(&mut names.walk()) {
            if ident.kind() != "identifier" {
                continue;
            }
            if let Some(name) = node_text(self.source, ident) {
                self.diagnostics.push(Diagnostic {
                    range: range_from_node(self.source, ident),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!("name `{name}` should not be declared with type `{ty}`"),
                    ..Diagnostic::default()
                });
            }
        }
    }

    fn validate_type_nodes(&mut self, node: Node<'_>) {
        match node.kind() {
            "type_expr" | "grouped_type" | "parameter_types" | "binder_parameters" => {
                for child in node.named_children(&mut node.walk()) {
                    self.validate_type_nodes(child);
                }
            }
            "binding" => {
                if let Some(ty) = node.child_by_field_name("type") {
                    self.validate_type_nodes(ty);
                }
            }
            "call" => {
                if let Some(function) = node.child_by_field_name("function") {
                    self.validate_type_nodes(function);
                }
                if let Some(arguments) = node.child_by_field_name("arguments") {
                    self.validate_type_nodes(arguments);
                }
            }
            "arguments" => {
                for child in node.named_children(&mut node.walk()) {
                    self.validate_type_nodes(child);
                }
            }
            "identifier" => {
                if let Some(name) = node_text(self.source, node) {
                    if !self.globals.types.contains(&name) {
                        self.diagnostics.push(Diagnostic {
                            range: range_from_node(self.source, node),
                            severity: Some(DiagnosticSeverity::WARNING),
                            message: format!("type `{name}` not declared"),
                            ..Diagnostic::default()
                        });
                    }
                }
            }
            _ => {}
        }
    }

    fn lookup_identifier(&self, env: &LocalEnv, name: &str) -> IdentifierLookup {
        if let Some(ty) = env.get(name) {
            return IdentifierLookup::Variable(ty.clone());
        }
        if let Some(value) = self.globals.names.get(name) {
            return IdentifierLookup::Name(value.type_name.clone());
        }
        if let Some(predicate) = self.globals.predicates.get(name) {
            if predicate.arity == 0 {
                return IdentifierLookup::ZeroArgPredicate;
            }
        }
        if let Some(function) = self.globals.functions.get(name) {
            if function.arity == 0 {
                return IdentifierLookup::ZeroArgFunction(function.result_type.clone());
            }
            return IdentifierLookup::Function {
                arity: function.arity,
            };
        }
        IdentifierLookup::Unknown
    }

    fn lookup_callable(&self, kind: SymbolKindRef, name: &str) -> Option<&CallableDecl> {
        match kind {
            SymbolKindRef::Function => self.globals.functions.get(name),
            SymbolKindRef::Process => self.globals.processes.get(name),
            SymbolKindRef::Event => self.globals.events.get(name),
            SymbolKindRef::Predicate => self.globals.predicates.get(name),
            SymbolKindRef::Table => self.globals.tables.get(name),
        }
    }

    fn callable_candidates(&self, kind: SymbolKindRef) -> Vec<String> {
        match kind {
            SymbolKindRef::Function => self.globals.functions.keys().cloned().collect(),
            SymbolKindRef::Process => process_candidates(&self.globals),
            SymbolKindRef::Event => self.globals.events.keys().cloned().collect(),
            SymbolKindRef::Predicate => self.globals.predicates.keys().cloned().collect(),
            SymbolKindRef::Table => self.globals.tables.keys().cloned().collect(),
        }
    }

    fn callable_label(&self, kind: SymbolKindRef) -> &'static str {
        match kind {
            SymbolKindRef::Function => "function",
            SymbolKindRef::Process => "process",
            SymbolKindRef::Event => "event",
            SymbolKindRef::Predicate => "predicate",
            SymbolKindRef::Table => "table",
        }
    }

    fn infer_expr_type(&self, node: Node<'_>, env: &LocalEnv) -> Option<String> {
        match node.kind() {
            "identifier" => {
                let name = node_text(self.source, node)?;
                match self.lookup_identifier(env, &name) {
                    IdentifierLookup::Variable(ty)
                    | IdentifierLookup::Name(ty)
                    | IdentifierLookup::ZeroArgFunction(ty) => ty,
                    IdentifierLookup::ZeroArgPredicate => Some("bool".into()),
                    IdentifierLookup::Function { .. } | IdentifierLookup::Unknown => None,
                }
            }
            "binding" => {
                if let Some(ty) = node.child_by_field_name("type") {
                    Some(type_text(self.source, ty))
                } else {
                    let name_node = node.child_by_field_name("name")?;
                    let name = node_text(self.source, name_node)?;
                    match self.lookup_identifier(env, &name) {
                        IdentifierLookup::Variable(ty)
                        | IdentifierLookup::Name(ty)
                        | IdentifierLookup::ZeroArgFunction(ty) => ty,
                        IdentifierLookup::ZeroArgPredicate => Some("bool".into()),
                        IdentifierLookup::Function { .. } | IdentifierLookup::Unknown => None,
                    }
                }
            }
            "new_binding" => node.child_by_field_name("type").map(|ty| type_text(self.source, ty)),
            "call" => {
                let function = node.child_by_field_name("function")?;
                let name = node_text(self.source, function)?;
                self.globals
                    .functions
                    .get(&name)
                    .or_else(|| self.globals.predicates.get(&name))
                    .and_then(|decl| decl.result_type.clone())
            }
            "new_name" => Some("bitstring".into()),
            "new_term" => {
                let binding = node.child_by_field_name("binding")?;
                let mut inner = env.clone();
                extend_with_binding(&mut inner, binding, self.source);
                let body = node.child_by_field_name("body")?;
                self.infer_expr_type(body, &inner)
            }
            "if_term" => {
                let consequence = node.child_by_field_name("consequence")?;
                let alternative = node.child_by_field_name("alternative")?;
                let left_ty = self.infer_expr_type(consequence, env);
                let right_ty = self.infer_expr_type(alternative, env);
                if left_ty == right_ty { left_ty } else { None }
            }
            "query_expr" => {
                let mut walk = node.walk();
                let mut children = node.named_children(&mut walk);
                let child = children.next()?;
                match child.kind() {
                    "prefix_query" => Some("bool".into()),
                    _ => self.infer_expr_type(child, env),
                }
            }
            "binary_expr" => {
                let op = operator_text(self.source, node)?;
                match op.as_str() {
                    "=" | "<>" | "<" | "<=" | ">" | ">=" | "&&" | "||" => Some("bool".into()),
                    "+" | "-" | "*" | "/" => self
                        .infer_expr_type(node.child_by_field_name("left")?, env)
                        .or_else(|| Some("nat".into())),
                    _ => None,
                }
            }
            "unary_expr" => {
                let op = operator_text(self.source, node)?;
                match op.as_str() {
                    "not" => Some("bool".into()),
                    _ => None,
                }
            }
            "string" | "parameter" => Some("bitstring".into()),
            "number" => Some("nat".into()),
            "boolean" => Some("bool".into()),
            "grouped_term" => node
                .named_children(&mut node.walk())
                .next()
                .and_then(|child| self.infer_expr_type(child, env)),
            "grouped_query" => node
                .named_children(&mut node.walk())
                .next()
                .and_then(|child| self.infer_expr_type(child, env)),
            "tuple" => Some("bitstring".into()),
            "choice_term" => {
                let left = node.child_by_field_name("left")?;
                let right = node.child_by_field_name("right")?;
                let left_ty = self.infer_expr_type(left, env);
                let right_ty = self.infer_expr_type(right, env);
                if left_ty == right_ty { left_ty } else { None }
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolKindRef {
    Function,
    Process,
    Event,
    Predicate,
    Table,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SymbolKindDecl {
    Type,
    Function,
    Process,
    Event,
    Predicate,
    Table,
    Name,
}

fn parameter_types(node: Option<Node<'_>>, source: &str) -> Vec<Option<String>> {
    let Some(node) = node else {
        return Vec::new();
    };
    node.named_children(&mut node.walk())
        .filter(|child| child.kind() == "type_expr")
        .map(|child| Some(type_text(source, child)))
        .collect()
}

fn binder_parameter_types(node: Node<'_>, source: &str) -> Vec<Option<String>> {
    let Some(parameters) = first_named_child_of_kind(node, "binder_parameters") else {
        return Vec::new();
    };
    parameters
        .named_children(&mut parameters.walk())
        .filter(|child| child.kind() == "binding")
        .map(|binding| binding.child_by_field_name("type").map(|ty| type_text(source, ty)))
        .collect()
}

fn extend_with_binder_parameters(env: &mut LocalEnv, node: Node<'_>, source: &str) {
    if let Some(parameters) = first_named_child_of_kind(node, "binder_parameters") {
        extend_with_bindings_from_children(env, parameters, source);
    }
}

fn extend_with_bindings_from_children(env: &mut LocalEnv, node: Node<'_>, source: &str) {
    for child in node.named_children(&mut node.walk()) {
        if child.kind() == "binding" {
            extend_with_binding(env, child, source);
        }
    }
}

fn extend_with_binding(env: &mut LocalEnv, binding: Node<'_>, source: &str) {
    let Some(name_node) = binding.child_by_field_name("name") else {
        return;
    };
    let ty = binding.child_by_field_name("type").map(|node| type_text(source, node));
    match name_node.kind() {
        "identifier" => {
            if let Some(name) = node_text(source, name_node) {
                env.insert(name, ty);
            }
        }
        "tuple_pattern" => extend_with_pattern(env, name_node, source),
        _ => {}
    }
}

fn extend_with_pattern_binding_like(env: &mut LocalEnv, binding: Node<'_>, source: &str) {
    match binding.kind() {
        "binding" => extend_with_binding(env, binding, source),
        "new_binding" => {
            let Some(name_node) = binding.child_by_field_name("name") else {
                return;
            };
            let ty = binding.child_by_field_name("type").map(|node| type_text(source, node));
            if let Some(name) = node_text(source, name_node) {
                env.insert(name, ty);
            }
        }
        _ => {}
    }
}

fn extend_with_pattern(env: &mut LocalEnv, node: Node<'_>, source: &str) {
    match node.kind() {
        "pattern" | "grouped_pattern" | "tuple_pattern" | "call_pattern" | "pattern_arguments" => {
            for child in node.named_children(&mut node.walk()) {
                extend_with_pattern(env, child, source);
            }
        }
        "binding" => extend_with_binding(env, node, source),
        _ => {}
    }
}

fn collect_argument_nodes(arguments: Node<'_>) -> Vec<Node<'_>> {
    arguments.named_children(&mut arguments.walk()).collect()
}

fn collect_pattern_nodes(arguments: Node<'_>) -> Vec<Node<'_>> {
    arguments.named_children(&mut arguments.walk()).collect()
}

fn count_call_arguments(arguments: Node<'_>) -> usize {
    arguments.named_children(&mut arguments.walk()).count()
}

fn type_text(source: &str, node: Node<'_>) -> String {
    snippet_for_node(source, node)
}

fn leading_keyword(source: &str, node: Node<'_>) -> Option<String> {
    let text = snippet_for_node(source, node);
    let keyword: String = text
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_')
        .collect();
    if keyword.is_empty() {
        None
    } else {
        Some(keyword)
    }
}

fn count_type_expr_children(node: Node<'_>) -> usize {
    node.named_children(&mut node.walk())
        .filter(|child| child.kind() == "type_expr")
        .count()
}

fn first_named_child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    node.named_children(&mut node.walk())
        .find(|child| child.kind() == kind)
}

fn first_named_descendant_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    if node.kind() == kind {
        return Some(node);
    }
    for child in node.named_children(&mut node.walk()) {
        if let Some(found) = first_named_descendant_of_kind(child, kind) {
            return Some(found);
        }
    }
    None
}

fn option_policy_for_owner(owner: Node<'_>, source: &str) -> Option<OptionPolicy> {
    match owner.kind() {
        "free_decl" => Some(OptionPolicy::FreeName),
        "in_process" | "get_process" => Some(OptionPolicy::ProcessInputGet),
        "query_decl" => owner
            .child_by_field_name("body")
            .filter(|body| contains_prefix_query_kind(*body, "secret", source))
            .map(|_| OptionPolicy::QuerySecret),
        _ => None,
    }
}

fn contains_prefix_query_kind(node: Node<'_>, prefix: &str, source: &str) -> bool {
    if node.kind() == "prefix_query" {
        return leading_keyword(source, node).as_deref() == Some(prefix);
    }
    node.named_children(&mut node.walk())
        .any(|child| contains_prefix_query_kind(child, prefix, source))
}

fn is_non_reference_identifier(node: Node<'_>, parent: Node<'_>) -> bool {
    match parent.kind() {
        "type_decl" | "type_expr" | "parameter_types" | "identifier_list" | "options" => true,
        "fun_decl" | "event_decl" | "pred_decl" | "table_decl" | "let_decl" | "letfun_decl"
        | "set_decl" | "binding" => true,
        "name_binding" => parent
            .child_by_field_name("name")
            .is_some_and(|field| field.id() == node.id()),
        "call_pattern" => parent
            .child_by_field_name("function")
            .is_some_and(|field| field.id() == node.id()),
        "new_name" => parent
            .child_by_field_name("name")
            .is_some_and(|field| field.id() == node.id()),
        "call" => parent
            .child_by_field_name("function")
            .is_some_and(|field| field.id() == node.id()),
        "call_process" => parent
            .child_by_field_name("name")
            .is_some_and(|field| field.id() == node.id()),
        "event_process" | "event_call" => true,
        _ => false,
    }
}

fn operator_text(source: &str, node: Node<'_>) -> Option<String> {
    let op = node.child_by_field_name("operator")?;
    node_text(source, op)
}

fn node_text(source: &str, node: Node<'_>) -> Option<String> {
    node.utf8_text(source.as_bytes()).ok().map(str::to_owned)
}

fn process_candidates(globals: &GlobalEnv) -> Vec<String> {
    let mut out: Vec<String> = globals.processes.keys().cloned().collect();
    out.extend(["in", "out", "new", "if", "event", "phase"].into_iter().map(str::to_owned));
    out.extend(["insert", "get"].into_iter().map(str::to_owned));
    out
}

fn identifier_candidates(env: &LocalEnv, globals: &GlobalEnv) -> Vec<String> {
    let mut out: Vec<String> = env.keys().cloned().collect();
    out.extend(globals.names.keys().cloned());
    out.extend(
        globals
            .functions
            .iter()
            .filter(|(_, decl)| decl.arity == 0)
            .map(|(name, _)| name.clone()),
    );
    out.extend(
        globals
            .predicates
            .iter()
            .filter(|(_, decl)| decl.arity == 0)
            .map(|(name, _)| name.clone()),
    );
    out
}

fn unknown_symbol_message(kind: &str, name: &str, candidates: &[String]) -> String {
    if let Some(suggestion) = nearest_name(name, candidates) {
        format!("unknown {kind} `{name}`; did you mean `{suggestion}`?")
    } else {
        format!("unknown {kind} `{name}`")
    }
}

fn nearest_name<'a>(name: &str, candidates: &'a [String]) -> Option<&'a str> {
    candidates
        .iter()
        .filter_map(|candidate| {
            let distance = edit_distance(name, candidate);
            (distance <= 2).then_some((distance, candidate.as_str()))
        })
        .min_by_key(|(distance, candidate)| (*distance, candidate.len()))
        .map(|(_, candidate)| candidate)
}

fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut prev: Vec<usize> = (0..=right.len()).collect();
    let mut curr = vec![0; right.len() + 1];

    for (i, lch) in left.iter().enumerate() {
        curr[0] = i + 1;
        for (j, rch) in right.iter().enumerate() {
            let cost = usize::from(lch != rch);
            curr[j + 1] = (prev[j + 1] + 1)
                .min(curr[j] + 1)
                .min(prev[j] + cost);
        }
        prev.clone_from(&curr);
    }

    prev[right.len()]
}

fn snippet_for_node(source: &str, node: Node<'_>) -> String {
    node.utf8_text(source.as_bytes())
        .ok()
        .map(|text| text.trim().chars().take(24).collect())
        .filter(|text: &String| !text.is_empty())
        .unwrap_or_else(|| node.kind().to_owned())
}

fn completion_item_priority(item: &CompletionItem) -> u8 {
    match item.detail.as_deref() {
        Some("constant") => 0,
        Some("variable") => 1,
        Some("function") => 2,
        Some("event") | Some("predicate") | Some("table") | Some("process") => 2,
        Some("type") => 3,
        Some("option") => 4,
        _ if item.kind == Some(CompletionItemKind::KEYWORD) => 10,
        _ => 5,
    }
}

fn completion_prefix(source: &str, position: Position) -> Option<String> {
    let offset = offset_for_position(source, position)?;
    let mut chars = Vec::new();
    for ch in source[..offset].chars().rev() {
        if !is_completion_prefix_char(ch) {
            break;
        }
        chars.push(ch);
    }
    chars.reverse();
    Some(chars.into_iter().collect())
}

fn is_completion_prefix_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '_' | '\'' | '-' | '@')
}

fn matches_prefix(candidate: &str, prefix: &str) -> bool {
    prefix.is_empty() || candidate.starts_with(prefix)
}

fn range_from_node(source: &str, node: Node<'_>) -> LspRange {
    let start = position_from_offset(source, node.start_byte());
    let end = position_from_offset(source, node.end_byte());
    LspRange { start, end }
}

pub fn position_from_offset(source: &str, offset: usize) -> Position {
    let mut line = 0u32;
    let mut col = 0u32;
    for (idx, ch) in source.char_indices() {
        if idx >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    Position::new(line, col)
}

pub fn offset_for_position(source: &str, position: Position) -> Option<usize> {
    let mut line = 0u32;
    let mut col = 0u32;
    for (idx, ch) in source.char_indices() {
        if line == position.line && col == position.character {
            return Some(idx);
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }

    if line == position.line && col == position.character {
        Some(source.len())
    } else {
        None
    }
}

pub fn range_from_bytes(source: &str, range: Range<usize>) -> LspRange {
    LspRange {
        start: position_from_offset(source, range.start),
        end: position_from_offset(source, range.end),
    }
}

fn language() -> Language {
    unsafe { tree_sitter_proverif() }
}

unsafe extern "C" {
    fn tree_sitter_proverif() -> Language;
}

#[cfg(test)]
mod tests {
    use super::parse;
    use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position};

    fn messages(source: &str) -> Vec<String> {
        parse(source)
            .expect("parse source")
            .diagnostics()
            .into_iter()
            .map(|diagnostic| diagnostic.message)
            .collect()
    }

    fn completion_labels(source: &str, position: Position) -> Vec<String> {
        parse(source)
            .expect("parse source")
            .completion_items(position)
            .into_iter()
            .map(|item| item.label)
            .collect()
    }

    fn completion_items(source: &str, position: Position) -> Vec<CompletionItem> {
        parse(source)
            .expect("parse source")
            .completion_items(position)
    }

    #[test]
    fn warns_for_unknown_process_typo() {
        let source = r#"
free c: channel.
process ut(c, "x")
"#;
        let messages = messages(source);
        assert!(messages.iter().any(|message| {
            message.contains("unknown process `ut`") && message.contains("did you mean `out`")
        }));
    }

    #[test]
    fn warns_for_reduc_function_in_query() {
        let source = r#"
reduc forall x:bitstring; h(x) = x.
query attacker(h("x")).
process 0
"#;
        let messages = messages(source);
        assert!(messages.iter().any(|message| {
            message.contains("function `h` is defined by `reduc`; it should not appear in a query")
        }));
    }

    #[test]
    fn checks_query_predicates() {
        let source = r#"
free c: channel.
free n: bitstring.
table seen(bitstring).
query mess(n, n).
query attacker(n, n).
query table(n).
query table(seen(n)).
process 0
"#;
        let messages = messages(source);
        assert!(messages.iter().any(|message| {
            message.contains("first argument of `mess` should have type channel")
        }));
        assert!(messages
            .iter()
            .any(|message| message.contains("predicate `attacker` expects 1 argument")));
        assert!(messages
            .iter()
            .any(|message| message.contains("`table` expects a table application")));
        assert!(!messages
            .iter()
            .any(|message| message.contains("unknown table `seen`")));
    }

    #[test]
    fn parses_query_binders_and_new_names() {
        let source = r#"
type host.
event endAparam(host, host).
event beginAparam(host, host).
query x: host, y: host; inj-event(endAparam(x,y)) ==> inj-event(beginAparam(x,y)).
not attacker(new Kas).
"#;
        let messages = messages(source);
        assert!(!messages
            .iter()
            .any(|message| message.contains("syntax error near")));
    }

    #[test]
    fn parses_get_insert_and_equality_input() {
        let source = r#"
free c: channel.
free A: bitstring.
type key.
type host.
table keys(host, key).
let processK =
  in(c, =A);
  get keys(=A, kas) in
  insert keys(A, kas).
"#;
        let messages = messages(source);
        assert!(!messages
            .iter()
            .any(|message| message.contains("syntax error near")));
    }

    #[test]
    fn resolves_global_names_when_parser_uses_binding_nodes() {
        let source = r#"
type skey.
type pkey.
type host.
type nonce.
fun pk(skey): pkey.
fun encrypt(bitstring, pkey): bitstring.
const A, B: host.
free c: channel.
free M: nonce [private].

let processA(pkB: pkey) =
  out(c, (A, encrypt((M), pkB))).
"#;
        let messages = messages(source);
        assert!(!messages.iter().any(|message| message.contains("unknown variable `M`")));
        assert!(!messages.iter().any(|message| message.contains("variable `M` not declared")));
    }

    #[test]
    fn checks_not_decl_with_binders() {
        let source = r#"
type host.
not x: host; attacker(x).
"#;
        let messages = messages(source);
        assert!(!messages.iter().any(|message| message.contains("unknown identifier `x`")));
        assert!(!messages.iter().any(|message| message.contains("variable `x` not declared")));
    }

    #[test]
    fn parses_and_checks_noninterf_among() {
        let source = r#"
fun hash(bitstring): bitstring.
free x, n: bitstring [private].
noninterf x among (n, hash(n)).
"#;
        let messages = messages(source);
        assert!(!messages.iter().any(|message| message.contains("syntax error near")));
        assert!(!messages.iter().any(|message| message.contains("unknown function `among`")));
    }

    #[test]
    fn checks_get_condition_against_upstream_rules() {
        let source = r#"
type key.
type host.
table keys(host, key).
let processK =
  get keys(h, k) suchthat new s in 0.
"#;
        let messages = messages(source);
        assert!(messages
            .iter()
            .any(|message| message.contains("get condition has type `bitstring`, expected `bool`")));
        assert!(messages
            .iter()
            .any(|message| message.contains("`new` is not allowed in conditions of `get`")));
    }

    #[test]
    fn propagates_env_through_replicated_inputs() {
        let source = r#"
free c: channel.
let p =
  ! in(c, x:bitstring);
  out(c, x).
"#;
        let messages = messages(source);
        assert!(!messages.iter().any(|message| message.contains("unknown identifier `x`")));
    }

    #[test]
    fn resolves_predicates_in_process_conditions() {
        let source = r#"
type host.
type idset.
pred memberid(host, idset).
free c: channel.
let p =
  in(c, (x:host, s:idset));
  if memberid(x, s) then
  out(c, x).
"#;
        let messages = messages(source);
        assert!(!messages.iter().any(|message| message.contains("unknown predicate `memberid`")));
        assert!(!messages.iter().any(|message| message.contains("unknown function `memberid`")));
        assert!(!messages
            .iter()
            .any(|message| message.contains("if condition has type")));
    }

    #[test]
    fn propagates_env_through_replicated_inputs_with_comments() {
        let source = r#"
type host.
free t: host.
fun channel_for_host(host): channel.
let p =
  (!
    (* comment *)
    in(channel_for_host(t), receivername: host);
    out(channel_for_host(receivername), t)
  ).
"#;
        let messages = messages(source);
        assert!(!messages
            .iter()
            .any(|message| message.contains("unknown identifier `receivername`")));
    }

    #[test]
    fn checks_weaksecret_requires_private_free_names() {
        let source = r#"
type passwd.
const cst: passwd.
free pubv: passwd.
free privv: passwd [private].
weaksecret cst.
weaksecret pubv.
weaksecret privv.
"#;
        let messages = messages(source);
        let count = messages
            .iter()
            .filter(|message| message.contains("weaksecret can only be tested on private free names"))
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn checks_noninterf_requires_private_free_names_and_matching_types() {
        let source = r#"
type nonce.
type host.
free pubn: nonce.
free privn: nonce [private].
noninterf pubn.
noninterf privn among ("x", "y").
noninterf privn among ("x", 0).
"#;
        let messages = messages(source);
        assert!(messages
            .iter()
            .any(|message| message.contains("noninterf can only be tested on private free names")));
        assert!(messages
            .iter()
            .any(|message| message.contains("noninterf value has type `nat`, expected `nonce`")));
    }

    #[test]
    fn checks_secret_options_and_putbegin_events() {
        let source = r#"
type nonce.
free k: nonce [private].
event e(nonce).
query secret k [bad].
query putbegin event:e, missing.
"#;
        let messages = messages(source);
        assert!(messages.iter().any(|message| {
            message.contains("the allowed options for query secret are reachability")
        }));
        assert!(messages
            .iter()
            .any(|message| message.contains("unknown event `missing`")));
    }

    #[test]
    fn checks_invalid_free_and_process_options() {
        let source = r#"
free k: bitstring [kokona].
free c: channel.
process in(c, x) [bad]
"#;
        let messages = messages(source);
        assert!(messages
            .iter()
            .any(|message| message.contains("for free names, the only allowed option is private")));
        assert!(messages
            .iter()
            .any(|message| message.contains("process input and get can only have \"precise\" as option")));
    }

    #[test]
    fn rejects_special_query_forms_in_lemmas_and_bare_phase_queries() {
        let source = r#"
free c: channel.
free k: bitstring [private].
lemma secret k.
query phase 1.
"#;
        let messages = messages(source);
        assert!(messages.iter().any(|message| {
            message.contains("lemmas, axioms, and restrictions should be correspondence queries")
        }));
        assert!(messages
            .iter()
            .any(|message| message.contains("phase can only be used with attacker, mess, or table")));
    }

    #[test]
    fn completion_uses_lookahead_for_top_level_keywords() {
        let labels = completion_labels("", Position::new(0, 0));
        assert!(labels.iter().any(|label| label == "type"));
        assert!(labels.iter().any(|label| label == "query"));
    }

    #[test]
    fn completion_suggests_declared_identifiers_when_expected() {
        let source = r#"
type nonce.
const secret: nonce.
query attacker(sec).
"#;
        let labels = completion_labels(source, Position::new(3, 18));
        assert!(labels.iter().any(|label| label == "secret"));
    }

    #[test]
    fn completion_suggests_builtin_types_for_type_positions() {
        let labels = completion_labels("free c:chan.", Position::new(0, 11));
        assert!(labels.iter().any(|label| label == "channel"));
    }

    #[test]
    fn completion_classifies_declared_symbols() {
        let source = r#"
type nonce.
fun hash(nonce): nonce.
const secret: nonce.
set flag = true.
query attacker().
"#;
        let items = completion_items(source, Position::new(5, 15));

        let ty = items.iter().find(|item| item.label == "nonce").expect("type suggestion");
        assert_eq!(ty.kind, Some(CompletionItemKind::CLASS));
        assert_eq!(ty.detail.as_deref(), Some("type"));

        let fun = items.iter().find(|item| item.label == "hash").expect("function suggestion");
        assert_eq!(fun.kind, Some(CompletionItemKind::FUNCTION));
        assert_eq!(fun.detail.as_deref(), Some("function"));

        let cst = items.iter().find(|item| item.label == "secret").expect("constant suggestion");
        assert_eq!(cst.kind, Some(CompletionItemKind::CONSTANT));
        assert_eq!(cst.detail.as_deref(), Some("constant"));

        let var = items.iter().find(|item| item.label == "flag").expect("variable suggestion");
        assert_eq!(var.kind, Some(CompletionItemKind::VARIABLE));
        assert_eq!(var.detail.as_deref(), Some("variable"));
    }

    #[test]
    fn completion_prioritizes_declared_symbols_over_keywords() {
        let source = r#"
type input_tag.
query attacker(i).
"#;
        let items = completion_items(source, Position::new(2, 16));
        let symbol_idx = items
            .iter()
            .position(|item| item.label == "input_tag")
            .expect("symbol suggestion");
        let keyword_idx = items
            .iter()
            .position(|item| item.kind == Some(CompletionItemKind::KEYWORD))
            .expect("keyword suggestion");
        assert!(symbol_idx < keyword_idx);
    }

    #[test]
    fn completion_suggests_private_option_for_free_names() {
        let labels = completion_labels("free k: bitstring [pri].", Position::new(0, 21));
        assert!(labels.iter().any(|label| label == "private"));
    }

    #[test]
    fn completion_suggests_declared_events() {
        let source = r#"
event begin(bitstring).
query inj-event(be).
"#;
        let items = completion_items(source, Position::new(2, 17));
        let event = items.iter().find(|item| item.label == "begin").expect("event suggestion");
        assert_eq!(event.detail.as_deref(), Some("event"));
    }

    #[test]
    fn completion_suggests_bound_variables() {
        let source = r#"
free c: channel.
let p =
  in(c, x:bitstring);
  out(c, x).
"#;
        let items = completion_items(source, Position::new(4, 9));
        let variable = items
            .iter()
            .find(|item| item.label == "x")
            .expect("bound variable suggestion");
        assert_eq!(variable.detail.as_deref(), Some("variable"));
    }
}
