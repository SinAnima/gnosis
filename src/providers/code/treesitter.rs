use crate::providers::util::{attrs, entity, ext_is, line_of, relationship};
use crate::{
    AnalysisResult, Confidence, ContentReader, Entity, KnowledgeRecord, ObjectDescriptor,
    ProtoData, ProviderId, Relationship, Result, Support, UnderstandingProvider,
    UnderstandingStatus,
};
use tree_sitter::{Node, Parser};

pub struct CppProvider;
pub struct RustProvider;
pub struct ElixirProvider;
pub struct JavaScriptProvider;
pub struct TypeScriptProvider;
pub struct PythonProvider;
pub struct JavaProvider;
pub struct CSharpProvider;

impl UnderstandingProvider for CppProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("tree-sitter-cpp")
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        if ext_is(
            &object.extension,
            &["cpp", "cc", "cxx", "hpp", "hh", "hxx", "h", "c"],
        ) {
            Support::Full
        } else {
            Support::None
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        _proto: &ProtoData,
        content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        analyze_language(
            self.id(),
            object,
            content,
            tree_sitter_cpp::LANGUAGE.into(),
            LanguageKind::Cpp,
        )
    }
}

impl UnderstandingProvider for RustProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("tree-sitter-rust")
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        if ext_is(&object.extension, &["rs"]) {
            Support::Full
        } else {
            Support::None
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        _proto: &ProtoData,
        content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        analyze_language(
            self.id(),
            object,
            content,
            tree_sitter_rust::LANGUAGE.into(),
            LanguageKind::Rust,
        )
    }
}

impl UnderstandingProvider for ElixirProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("tree-sitter-elixir")
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        if ext_is(&object.extension, &["ex", "exs"]) {
            Support::Full
        } else {
            Support::None
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        _proto: &ProtoData,
        content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        analyze_language(
            self.id(),
            object,
            content,
            tree_sitter_elixir::LANGUAGE.into(),
            LanguageKind::Elixir,
        )
    }
}

impl UnderstandingProvider for JavaScriptProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("tree-sitter-javascript")
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        if ext_is(&object.extension, &["js", "mjs", "cjs"]) {
            Support::Full
        } else {
            Support::None
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        _proto: &ProtoData,
        content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        analyze_language(
            self.id(),
            object,
            content,
            tree_sitter_javascript::LANGUAGE.into(),
            LanguageKind::JavaScript,
        )
    }
}

impl UnderstandingProvider for TypeScriptProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("tree-sitter-typescript")
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        if ext_is(&object.extension, &["ts", "tsx", "mts", "cts"]) {
            Support::Full
        } else {
            Support::None
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        _proto: &ProtoData,
        content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        let language = if object.extension.as_deref() == Some("tsx") {
            tree_sitter_typescript::LANGUAGE_TSX.into()
        } else {
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
        };
        analyze_language(
            self.id(),
            object,
            content,
            language,
            LanguageKind::TypeScript,
        )
    }
}

impl UnderstandingProvider for PythonProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("tree-sitter-python")
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        if ext_is(&object.extension, &["py"]) {
            Support::Full
        } else {
            Support::None
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        _proto: &ProtoData,
        content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        analyze_language(
            self.id(),
            object,
            content,
            tree_sitter_python::LANGUAGE.into(),
            LanguageKind::Python,
        )
    }
}

impl UnderstandingProvider for JavaProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("tree-sitter-java")
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        if ext_is(&object.extension, &["java"]) {
            Support::Full
        } else {
            Support::None
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        _proto: &ProtoData,
        content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        analyze_language(
            self.id(),
            object,
            content,
            tree_sitter_java::LANGUAGE.into(),
            LanguageKind::Java,
        )
    }
}

impl UnderstandingProvider for CSharpProvider {
    fn id(&self) -> ProviderId {
        ProviderId::new("tree-sitter-csharp")
    }

    fn probe(&self, object: &ObjectDescriptor, _proto: &ProtoData) -> Support {
        if ext_is(&object.extension, &["cs"]) {
            Support::Full
        } else {
            Support::None
        }
    }

    fn analyze(
        &self,
        object: &ObjectDescriptor,
        _proto: &ProtoData,
        content: &mut dyn ContentReader,
    ) -> Result<AnalysisResult> {
        analyze_language(
            self.id(),
            object,
            content,
            tree_sitter_c_sharp::LANGUAGE.into(),
            LanguageKind::CSharp,
        )
    }
}

#[derive(Clone, Copy)]
enum LanguageKind {
    Cpp,
    Rust,
    Elixir,
    JavaScript,
    TypeScript,
    Python,
    Java,
    CSharp,
}

fn analyze_language(
    provider: ProviderId,
    object: &ObjectDescriptor,
    content: &mut dyn ContentReader,
    language: tree_sitter::Language,
    kind: LanguageKind,
) -> Result<AnalysisResult> {
    let mut buf = Vec::new();
    content.read_to_end(&mut buf)?;
    let source = String::from_utf8_lossy(&buf);
    let source_str = source.as_ref();

    let mut parser = Parser::new();
    parser
        .set_language(&language)
        .map_err(|e| crate::GnosisError::provider(provider.as_str(), e.to_string()))?;

    let tree = parser.parse(source_str, None).ok_or_else(|| {
        crate::GnosisError::provider(provider.as_str(), "tree-sitter parse returned None")
    })?;

    let file_entity = entity(
        "module",
        &object
            .relative_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into()),
        &object.relative_path,
        &object.id,
        &provider,
        1,
        attrs(&[
            ("language", language_name(kind)),
            ("path", &object.relative_path.to_string_lossy()),
        ]),
        Confidence::High,
    );

    let mut entities = vec![file_entity.clone()];
    let mut relationships = Vec::new();

    walk(
        tree.root_node(),
        source_str,
        &provider,
        object,
        kind,
        &file_entity,
        &mut entities,
        &mut relationships,
    );

    let status = if entities.len() > 1 || !relationships.is_empty() {
        UnderstandingStatus::Understood
    } else {
        UnderstandingStatus::PartiallyUnderstood
    };

    Ok(AnalysisResult {
        record: KnowledgeRecord {
            entities,
            relationships,
            diagnostics: Vec::new(),
            status: Some(status),
            classification_reason: Some(format!("tree-sitter {}", language_name(kind))),
        },
        status,
        classification_reason: Some(format!("parsed with tree-sitter {}", language_name(kind))),
    })
}

fn language_name(kind: LanguageKind) -> &'static str {
    match kind {
        LanguageKind::Cpp => "cpp",
        LanguageKind::Rust => "rust",
        LanguageKind::Elixir => "elixir",
        LanguageKind::JavaScript => "javascript",
        LanguageKind::TypeScript => "typescript",
        LanguageKind::Python => "python",
        LanguageKind::Java => "java",
        LanguageKind::CSharp => "csharp",
    }
}

fn walk(
    node: Node,
    source: &str,
    provider: &ProviderId,
    object: &ObjectDescriptor,
    kind: LanguageKind,
    file_entity: &Entity,
    entities: &mut Vec<Entity>,
    relationships: &mut Vec<Relationship>,
) {
    match kind {
        LanguageKind::Cpp => extract_cpp(
            node,
            source,
            provider,
            object,
            file_entity,
            entities,
            relationships,
        ),
        LanguageKind::Rust => extract_rust(
            node,
            source,
            provider,
            object,
            file_entity,
            entities,
            relationships,
        ),
        LanguageKind::Elixir => extract_elixir(
            node,
            source,
            provider,
            object,
            file_entity,
            entities,
            relationships,
        ),
        LanguageKind::JavaScript | LanguageKind::TypeScript => extract_script(
            node,
            source,
            provider,
            object,
            kind,
            file_entity,
            entities,
            relationships,
        ),
        LanguageKind::Python => extract_python(
            node,
            source,
            provider,
            object,
            file_entity,
            entities,
            relationships,
        ),
        LanguageKind::Java => extract_java(
            node,
            source,
            provider,
            object,
            file_entity,
            entities,
            relationships,
        ),
        LanguageKind::CSharp => extract_csharp(
            node,
            source,
            provider,
            object,
            file_entity,
            entities,
            relationships,
        ),
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(
            child,
            source,
            provider,
            object,
            kind,
            file_entity,
            entities,
            relationships,
        );
    }
}

fn node_text<'a>(node: Node, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

fn child_by_field<'a>(node: Node<'a>, field: &str) -> Option<Node<'a>> {
    node.child_by_field_name(field)
}

fn extract_cpp(
    node: Node,
    source: &str,
    provider: &ProviderId,
    object: &ObjectDescriptor,
    file_entity: &Entity,
    entities: &mut Vec<Entity>,
    relationships: &mut Vec<Relationship>,
) {
    let path = &object.relative_path;
    let line = line_of(source, node.start_byte());
    match node.kind() {
        "preproc_include" => {
            let text = node_text(node, source);
            let include = text
                .trim()
                .trim_start_matches("#include")
                .trim()
                .trim_matches(|c| c == '"' || c == '<' || c == '>');
            if !include.is_empty() {
                let target = entity(
                    "module",
                    include,
                    path,
                    &object.id,
                    provider,
                    line,
                    attrs(&[("include", include)]),
                    Confidence::Medium,
                );
                relationships.push(relationship(
                    "includes",
                    &file_entity.id,
                    &target.id,
                    provider,
                    path,
                    line,
                    Confidence::High,
                ));
                entities.push(target);
            }
        }
        "class_specifier" | "struct_specifier" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source);
                let kind = if node.kind().starts_with("class") {
                    "class"
                } else {
                    "struct"
                };
                let e = entity(
                    kind,
                    name,
                    path,
                    &object.id,
                    provider,
                    line,
                    attrs(&[("language", "cpp")]),
                    Confidence::High,
                );
                relationships.push(relationship(
                    "defines",
                    &file_entity.id,
                    &e.id,
                    provider,
                    path,
                    line,
                    Confidence::High,
                ));
                // base classes
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "base_class_clause" {
                        let base_txt = node_text(child, source);
                        for base in base_txt.split([',', ':']).map(str::trim) {
                            let base = base
                                .trim_start_matches("public")
                                .trim_start_matches("private")
                                .trim_start_matches("protected")
                                .trim();
                            if base.is_empty() || base == "public" {
                                continue;
                            }
                            let base_ent = entity(
                                "class",
                                base,
                                path,
                                &object.id,
                                provider,
                                line,
                                attrs(&[("role", "base")]),
                                Confidence::Medium,
                            );
                            relationships.push(relationship(
                                "inherits",
                                &e.id,
                                &base_ent.id,
                                provider,
                                path,
                                line,
                                Confidence::Medium,
                            ));
                            entities.push(base_ent);
                        }
                    }
                }
                entities.push(e);
            }
        }
        "function_definition" | "declaration" => {
            if let Some(declarator) = child_by_field(node, "declarator") {
                if let Some(name) = find_identifier(declarator, source) {
                    let kind = if node.kind() == "function_definition" {
                        "function"
                    } else if looks_like_function_decl(declarator) {
                        "function"
                    } else {
                        return;
                    };
                    let e = entity(
                        kind,
                        name,
                        path,
                        &object.id,
                        provider,
                        line,
                        attrs(&[("language", "cpp")]),
                        Confidence::High,
                    );
                    relationships.push(relationship(
                        "defines",
                        &file_entity.id,
                        &e.id,
                        provider,
                        path,
                        line,
                        Confidence::High,
                    ));
                    entities.push(e);
                }
            }
        }
        "namespace_definition" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source);
                let e = entity(
                    "namespace",
                    name,
                    path,
                    &object.id,
                    provider,
                    line,
                    attrs(&[("language", "cpp")]),
                    Confidence::High,
                );
                relationships.push(relationship(
                    "defines",
                    &file_entity.id,
                    &e.id,
                    provider,
                    path,
                    line,
                    Confidence::High,
                ));
                entities.push(e);
            }
        }
        "enum_specifier" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source);
                let e = entity(
                    "enum",
                    name,
                    path,
                    &object.id,
                    provider,
                    line,
                    attrs(&[("language", "cpp")]),
                    Confidence::High,
                );
                relationships.push(relationship(
                    "defines",
                    &file_entity.id,
                    &e.id,
                    provider,
                    path,
                    line,
                    Confidence::High,
                ));
                entities.push(e);
            }
        }
        _ => {}
    }
}

fn find_identifier<'a>(node: Node<'a>, source: &'a str) -> Option<&'a str> {
    if node.kind() == "identifier"
        || node.kind() == "type_identifier"
        || node.kind() == "field_identifier"
    {
        return Some(node_text(node, source));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(id) = find_identifier(child, source) {
            return Some(id);
        }
    }
    None
}

fn looks_like_function_decl(node: Node) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind().contains("function") || child.kind() == "parameter_list" {
            return true;
        }
        if looks_like_function_decl(child) {
            return true;
        }
    }
    false
}

fn extract_rust(
    node: Node,
    source: &str,
    provider: &ProviderId,
    object: &ObjectDescriptor,
    file_entity: &Entity,
    entities: &mut Vec<Entity>,
    relationships: &mut Vec<Relationship>,
) {
    let path = &object.relative_path;
    let line = line_of(source, node.start_byte());
    match node.kind() {
        "function_item" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source);
                let e = entity(
                    "function",
                    name,
                    path,
                    &object.id,
                    provider,
                    line,
                    attrs(&[("language", "rust")]),
                    Confidence::High,
                );
                relationships.push(relationship(
                    "defines",
                    &file_entity.id,
                    &e.id,
                    provider,
                    path,
                    line,
                    Confidence::High,
                ));
                entities.push(e);
            }
        }
        "struct_item" | "enum_item" | "trait_item" | "union_item" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source);
                let kind = match node.kind() {
                    "struct_item" => "struct",
                    "enum_item" => "enum",
                    "trait_item" => "trait",
                    _ => "type",
                };
                let e = entity(
                    kind,
                    name,
                    path,
                    &object.id,
                    provider,
                    line,
                    attrs(&[("language", "rust")]),
                    Confidence::High,
                );
                relationships.push(relationship(
                    "defines",
                    &file_entity.id,
                    &e.id,
                    provider,
                    path,
                    line,
                    Confidence::High,
                ));
                entities.push(e);
            }
        }
        "mod_item" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let name = node_text(name_node, source);
                let e = entity(
                    "module",
                    name,
                    path,
                    &object.id,
                    provider,
                    line,
                    attrs(&[("language", "rust")]),
                    Confidence::High,
                );
                relationships.push(relationship(
                    "defines",
                    &file_entity.id,
                    &e.id,
                    provider,
                    path,
                    line,
                    Confidence::High,
                ));
                entities.push(e);
            }
        }
        "use_declaration" | "extern_crate_declaration" => {
            let text = node_text(node, source);
            let import = text
                .trim()
                .trim_start_matches("use")
                .trim_start_matches("extern crate")
                .trim()
                .trim_end_matches(';')
                .trim();
            if !import.is_empty() {
                let target = entity(
                    "module",
                    import,
                    path,
                    &object.id,
                    provider,
                    line,
                    attrs(&[("import", import)]),
                    Confidence::Medium,
                );
                relationships.push(relationship(
                    "imports",
                    &file_entity.id,
                    &target.id,
                    provider,
                    path,
                    line,
                    Confidence::High,
                ));
                entities.push(target);
            }
        }
        "impl_item" => {
            if let Some(type_node) = child_by_field(node, "type") {
                let name = node_text(type_node, source);
                let e = entity(
                    "impl",
                    name,
                    path,
                    &object.id,
                    provider,
                    line,
                    attrs(&[("language", "rust")]),
                    Confidence::High,
                );
                relationships.push(relationship(
                    "defines",
                    &file_entity.id,
                    &e.id,
                    provider,
                    path,
                    line,
                    Confidence::High,
                ));
                if let Some(trait_node) = child_by_field(node, "trait") {
                    let trait_name = node_text(trait_node, source);
                    let trait_ent = entity(
                        "trait",
                        trait_name,
                        path,
                        &object.id,
                        provider,
                        line,
                        attrs(&[("role", "implemented")]),
                        Confidence::Medium,
                    );
                    relationships.push(relationship(
                        "implements",
                        &e.id,
                        &trait_ent.id,
                        provider,
                        path,
                        line,
                        Confidence::Medium,
                    ));
                    entities.push(trait_ent);
                }
                entities.push(e);
            }
        }
        _ => {}
    }
}

fn extract_elixir(
    node: Node,
    source: &str,
    provider: &ProviderId,
    object: &ObjectDescriptor,
    file_entity: &Entity,
    entities: &mut Vec<Entity>,
    relationships: &mut Vec<Relationship>,
) {
    let path = &object.relative_path;
    let line = line_of(source, node.start_byte());
    match node.kind() {
        "call" => {
            let text = node_text(node, source);
            let trimmed = text.trim_start();
            if let Some(rest) = trimmed.strip_prefix("defmodule ") {
                let name = rest
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .trim_end_matches(',')
                    .trim_end_matches(" do");
                if !name.is_empty() {
                    let e = entity(
                        "module",
                        name,
                        path,
                        &object.id,
                        provider,
                        line,
                        attrs(&[("language", "elixir")]),
                        Confidence::High,
                    );
                    relationships.push(relationship(
                        "defines",
                        &file_entity.id,
                        &e.id,
                        provider,
                        path,
                        line,
                        Confidence::High,
                    ));
                    entities.push(e);
                }
            } else if let Some(rest) = trimmed.strip_prefix("def ") {
                let name = rest
                    .split(|c: char| c == '(' || c.is_whitespace())
                    .next()
                    .unwrap_or("")
                    .trim();
                if !name.is_empty() {
                    let e = entity(
                        "function",
                        name,
                        path,
                        &object.id,
                        provider,
                        line,
                        attrs(&[("language", "elixir")]),
                        Confidence::High,
                    );
                    relationships.push(relationship(
                        "defines",
                        &file_entity.id,
                        &e.id,
                        provider,
                        path,
                        line,
                        Confidence::High,
                    ));
                    entities.push(e);
                }
            } else if let Some(rest) = trimmed.strip_prefix("defp ") {
                let name = rest
                    .split(|c: char| c == '(' || c.is_whitespace())
                    .next()
                    .unwrap_or("")
                    .trim();
                if !name.is_empty() {
                    let e = entity(
                        "function",
                        name,
                        path,
                        &object.id,
                        provider,
                        line,
                        attrs(&[("language", "elixir"), ("visibility", "private")]),
                        Confidence::High,
                    );
                    relationships.push(relationship(
                        "defines",
                        &file_entity.id,
                        &e.id,
                        provider,
                        path,
                        line,
                        Confidence::High,
                    ));
                    entities.push(e);
                }
            } else if trimmed.starts_with("use ")
                || trimmed.starts_with("import ")
                || trimmed.starts_with("alias ")
                || trimmed.starts_with("require ")
            {
                let kind = trimmed.split_whitespace().next().unwrap_or("import");
                let target_name = trimmed
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .trim_end_matches(',');
                if !target_name.is_empty() {
                    let target = entity(
                        "module",
                        target_name,
                        path,
                        &object.id,
                        provider,
                        line,
                        attrs(&[(kind, target_name)]),
                        Confidence::Medium,
                    );
                    relationships.push(relationship(
                        kind,
                        &file_entity.id,
                        &target.id,
                        provider,
                        path,
                        line,
                        Confidence::Medium,
                    ));
                    entities.push(target);
                }
            }
        }
        _ => {}
    }
}

fn push_named(
    kind: &str,
    name: &str,
    language: &str,
    source: &str,
    node: Node,
    provider: &ProviderId,
    object: &ObjectDescriptor,
    file_entity: &Entity,
    entities: &mut Vec<Entity>,
    relationships: &mut Vec<Relationship>,
) {
    if name.is_empty() {
        return;
    }
    let path = &object.relative_path;
    let line = line_of(source, node.start_byte());
    let named = entity(
        kind,
        name,
        path,
        &object.id,
        provider,
        line,
        attrs(&[("language", language)]),
        Confidence::High,
    );
    relationships.push(relationship(
        "defines",
        &file_entity.id,
        &named.id,
        provider,
        path,
        line,
        Confidence::High,
    ));
    entities.push(named);
}

fn push_import(
    text: &str,
    language: &str,
    source: &str,
    node: Node,
    provider: &ProviderId,
    object: &ObjectDescriptor,
    file_entity: &Entity,
    entities: &mut Vec<Entity>,
    relationships: &mut Vec<Relationship>,
) {
    let name = text
        .split_whitespace()
        .take(8)
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        return;
    }
    let path = &object.relative_path;
    let line = line_of(source, node.start_byte());
    let target = entity(
        "module",
        &name,
        path,
        &object.id,
        provider,
        line,
        attrs(&[("language", language)]),
        Confidence::Medium,
    );
    relationships.push(relationship(
        "imports",
        &file_entity.id,
        &target.id,
        provider,
        path,
        line,
        Confidence::Medium,
    ));
    entities.push(target);
}

fn extract_script(
    node: Node,
    source: &str,
    provider: &ProviderId,
    object: &ObjectDescriptor,
    kind: LanguageKind,
    file_entity: &Entity,
    entities: &mut Vec<Entity>,
    relationships: &mut Vec<Relationship>,
) {
    let language = language_name(kind);
    match node.kind() {
        "function_declaration" | "generator_function_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                push_named(
                    "function",
                    node_text(name_node, source),
                    language,
                    source,
                    node,
                    provider,
                    object,
                    file_entity,
                    entities,
                    relationships,
                );
            }
        }
        "method_definition" => {
            if let Some(name_node) = child_by_field(node, "name") {
                push_named(
                    "method",
                    node_text(name_node, source),
                    language,
                    source,
                    node,
                    provider,
                    object,
                    file_entity,
                    entities,
                    relationships,
                );
            }
        }
        "class_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                push_named(
                    "class",
                    node_text(name_node, source),
                    language,
                    source,
                    node,
                    provider,
                    object,
                    file_entity,
                    entities,
                    relationships,
                );
            }
        }
        "interface_declaration" | "type_alias_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let entity_kind = if node.kind() == "interface_declaration" {
                    "interface"
                } else {
                    "type"
                };
                push_named(
                    entity_kind,
                    node_text(name_node, source),
                    language,
                    source,
                    node,
                    provider,
                    object,
                    file_entity,
                    entities,
                    relationships,
                );
            }
        }
        "import_statement" => {
            push_import(
                node_text(node, source),
                language,
                source,
                node,
                provider,
                object,
                file_entity,
                entities,
                relationships,
            );
        }
        _ => {}
    }
}

fn extract_python(
    node: Node,
    source: &str,
    provider: &ProviderId,
    object: &ObjectDescriptor,
    file_entity: &Entity,
    entities: &mut Vec<Entity>,
    relationships: &mut Vec<Relationship>,
) {
    match node.kind() {
        "function_definition" => {
            if let Some(name_node) = child_by_field(node, "name") {
                push_named(
                    "function",
                    node_text(name_node, source),
                    "python",
                    source,
                    node,
                    provider,
                    object,
                    file_entity,
                    entities,
                    relationships,
                );
            }
        }
        "class_definition" => {
            if let Some(name_node) = child_by_field(node, "name") {
                push_named(
                    "class",
                    node_text(name_node, source),
                    "python",
                    source,
                    node,
                    provider,
                    object,
                    file_entity,
                    entities,
                    relationships,
                );
            }
        }
        "import_statement" | "import_from_statement" => {
            push_import(
                node_text(node, source),
                "python",
                source,
                node,
                provider,
                object,
                file_entity,
                entities,
                relationships,
            );
        }
        _ => {}
    }
}

fn extract_java(
    node: Node,
    source: &str,
    provider: &ProviderId,
    object: &ObjectDescriptor,
    file_entity: &Entity,
    entities: &mut Vec<Entity>,
    relationships: &mut Vec<Relationship>,
) {
    match node.kind() {
        "method_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                push_named(
                    "method",
                    node_text(name_node, source),
                    "java",
                    source,
                    node,
                    provider,
                    object,
                    file_entity,
                    entities,
                    relationships,
                );
            }
        }
        "class_declaration" | "interface_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let kind = if node.kind() == "class_declaration" {
                    "class"
                } else {
                    "interface"
                };
                push_named(
                    kind,
                    node_text(name_node, source),
                    "java",
                    source,
                    node,
                    provider,
                    object,
                    file_entity,
                    entities,
                    relationships,
                );
            }
        }
        "import_declaration" => {
            push_import(
                node_text(node, source),
                "java",
                source,
                node,
                provider,
                object,
                file_entity,
                entities,
                relationships,
            );
        }
        _ => {}
    }
}

fn extract_csharp(
    node: Node,
    source: &str,
    provider: &ProviderId,
    object: &ObjectDescriptor,
    file_entity: &Entity,
    entities: &mut Vec<Entity>,
    relationships: &mut Vec<Relationship>,
) {
    match node.kind() {
        "method_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                push_named(
                    "method",
                    node_text(name_node, source),
                    "csharp",
                    source,
                    node,
                    provider,
                    object,
                    file_entity,
                    entities,
                    relationships,
                );
            }
        }
        "class_declaration" | "interface_declaration" => {
            if let Some(name_node) = child_by_field(node, "name") {
                let kind = if node.kind() == "class_declaration" {
                    "class"
                } else {
                    "interface"
                };
                push_named(
                    kind,
                    node_text(name_node, source),
                    "csharp",
                    source,
                    node,
                    provider,
                    object,
                    file_entity,
                    entities,
                    relationships,
                );
            }
        }
        "using_directive" => {
            push_import(
                node_text(node, source),
                "csharp",
                source,
                node,
                provider,
                object,
                file_entity,
                entities,
                relationships,
            );
        }
        _ => {}
    }
}

#[cfg(test)]
mod language_tests {
    use super::*;
    use crate::ids::ObjectId;
    use std::path::PathBuf;

    fn object(rel: &str) -> ObjectDescriptor {
        let path = PathBuf::from(rel);
        ObjectDescriptor {
            id: ObjectId::new(format!("obj:{rel}")),
            path: path.clone(),
            relative_path: path.clone(),
            is_dir: false,
            size: 32,
            modified: None,
            media_type: "text/plain".into(),
            extension: path
                .extension()
                .map(|ext| ext.to_string_lossy().to_string()),
            etag: None,
        }
    }

    fn names(
        provider: &dyn UnderstandingProvider,
        rel: &str,
        source: &str,
    ) -> Vec<(String, String)> {
        let object = object(rel);
        let proto = ProtoData::default();
        let mut reader = crate::BytesContentReader::new(source.as_bytes().to_vec());
        let result = provider.analyze(&object, &proto, &mut reader).unwrap();
        result
            .record
            .entities
            .into_iter()
            .map(|entity| (entity.kind, entity.name))
            .collect()
    }

    #[test]
    fn javascript_typescript_python_java_and_csharp_extract_symbols() {
        let js = names(
            &JavaScriptProvider,
            "app.js",
            "import ledger from './ledger.js';\nfunction price(x) { return x; }\nclass Quote {}\n",
        );
        assert!(js
            .iter()
            .any(|(kind, name)| kind == "function" && name == "price"));
        assert!(js
            .iter()
            .any(|(kind, name)| kind == "class" && name == "Quote"));

        let ts = names(
            &TypeScriptProvider,
            "app.ts",
            "export function price(x: number) { return x; }\nexport class Quote {}\n",
        );
        assert!(ts
            .iter()
            .any(|(kind, name)| kind == "function" && name == "price"));

        let py = names(
            &PythonProvider,
            "price.py",
            "import ledger\n\ndef price(x):\n    return x\n\nclass Quote:\n    pass\n",
        );
        assert!(py
            .iter()
            .any(|(kind, name)| kind == "function" && name == "price"));
        assert!(py
            .iter()
            .any(|(kind, name)| kind == "class" && name == "Quote"));

        let java = names(
            &JavaProvider,
            "Quote.java",
            "import ledger.Entry;\nclass Quote { int price(int x) { return x; } }\n",
        );
        assert!(java
            .iter()
            .any(|(kind, name)| kind == "class" && name == "Quote"));
        assert!(java
            .iter()
            .any(|(kind, name)| kind == "method" && name == "price"));

        let cs = names(
            &CSharpProvider,
            "Quote.cs",
            "using Ledger;\nclass Quote { int Price(int x) { return x; } }\n",
        );
        assert!(cs
            .iter()
            .any(|(kind, name)| kind == "class" && name == "Quote"));
        assert!(cs
            .iter()
            .any(|(kind, name)| kind == "method" && name == "Price"));
    }
}
