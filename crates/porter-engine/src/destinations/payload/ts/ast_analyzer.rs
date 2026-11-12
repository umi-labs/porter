use super::errors::Result;
use super::schema::{
    BlockDefinition, BlockLabels, FieldAdmin, FieldDefinition, FieldType, ImportInfo, SelectOption,
    SourceLocation, TabDefinition,
};
use std::collections::HashMap;
use swc_ecma_ast::*;

#[derive(Clone)]
pub struct AstAnalyzer {
    pub fields: Vec<FieldDefinition>,
    pub blocks: HashMap<String, BlockDefinition>,
    pub imports: Vec<ImportInfo>,
    pub exports: HashMap<String, ExportedItem>,
}

#[derive(Debug, Clone)]
pub enum ExportedItem {
    Field(FieldDefinition),
    Fields(Vec<FieldDefinition>),
    Block(BlockDefinition),
    Function(String, Module), // function name and its AST
    Object(ObjectLit),
    Array(ArrayLit),
}

impl Default for AstAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl AstAnalyzer {
    pub fn new() -> Self {
        Self {
            fields: Vec::new(),
            blocks: HashMap::new(),
            imports: Vec::new(),
            exports: HashMap::new(),
        }
    }

    pub fn analyze_module(&mut self, module: &Module) -> Result<()> {
        for item in &module.body {
            self.analyze_module_item(item)?;
        }
        let _field_configs = self
            .exports
            .values()
            .filter(|item| matches!(item, ExportedItem::Object(_)))
            .count();
        Ok(())
    }

    fn analyze_module_item(&mut self, item: &ModuleItem) -> Result<()> {
        match item {
            ModuleItem::ModuleDecl(decl) => self.analyze_module_decl(decl)?,
            ModuleItem::Stmt(stmt) => self.analyze_statement(stmt)?,
        }
        Ok(())
    }

    fn analyze_module_decl(&mut self, decl: &ModuleDecl) -> Result<()> {
        match decl {
            ModuleDecl::Import(import_decl) => {
                self.extract_import(import_decl)?;
            }
            ModuleDecl::ExportDecl(export_decl) => {
                self.analyze_export_decl(export_decl)?;
            }
            ModuleDecl::ExportNamed(named_export) => {
                self.analyze_named_export(named_export)?;
            }
            ModuleDecl::ExportDefaultDecl(default_export) => {
                self.analyze_default_export(default_export)?;
            }
            ModuleDecl::ExportDefaultExpr(expr_export) => {
                self.analyze_default_expr_export(expr_export)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn extract_import(&mut self, import: &ImportDecl) -> Result<()> {
        let source = import.src.value.to_atom_lossy().to_string();
        let mut specifiers = Vec::new();

        for spec in &import.specifiers {
            match spec {
                ImportSpecifier::Named(named) => {
                    let imported = named
                        .imported
                        .as_ref()
                        .map(|n| self.get_module_export_name(n))
                        .unwrap_or_else(|| named.local.sym.to_string());
                    let local = named.local.sym.to_string();

                    specifiers.push(super::schema::ImportSpecifier::Named { imported, local });
                }
                ImportSpecifier::Default(default) => {
                    let local = default.local.sym.to_string();
                    specifiers.push(super::schema::ImportSpecifier::Default(local));
                }
                ImportSpecifier::Namespace(ns) => {
                    let local = ns.local.sym.to_string();
                    specifiers.push(super::schema::ImportSpecifier::Namespace(local));
                }
            }
        }

        self.imports.push(ImportInfo {
            source,
            specifiers,
            resolved_path: None,
        });

        Ok(())
    }

    fn analyze_statement(&mut self, stmt: &Stmt) -> Result<()> {
        if let Stmt::Decl(decl) = stmt {
            match decl {
                Decl::Var(var_decl) => {
                    for declarator in &var_decl.decls {
                        self.analyze_var_declarator(declarator)?;
                    }
                }
                Decl::Fn(fn_decl) => {
                    let name = fn_decl.ident.sym.to_string();
                    self.exports.insert(
                        name.clone(),
                        ExportedItem::Function(
                            name,
                            Module {
                                span: Default::default(),
                                body: vec![ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl.clone())))],
                                shebang: None,
                            },
                        ),
                    );
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn analyze_export_decl(&mut self, export: &ExportDecl) -> Result<()> {
        match &export.decl {
            Decl::Var(var_decl) => {
                for decl in &var_decl.decls {
                    self.analyze_var_declarator(decl)?;
                }
            }
            Decl::Fn(fn_decl) => {
                let name = fn_decl.ident.sym.to_string();
                self.exports.insert(
                    name.clone(),
                    ExportedItem::Function(
                        fn_decl.ident.sym.to_string(),
                        Module {
                            span: Default::default(),
                            body: vec![ModuleItem::Stmt(Stmt::Decl(Decl::Fn(fn_decl.clone())))],
                            shebang: None,
                        },
                    ),
                );
            }
            Decl::Class(_class_decl) => {}
            _ => {}
        }
        Ok(())
    }

    fn analyze_named_export(&mut self, export: &NamedExport) -> Result<()> {
        if let Some(_src) = &export.src {}

        for spec in &export.specifiers {
            match spec {
                ExportSpecifier::Named(named) => {
                    let _orig = self.get_module_export_name(&named.orig);
                }
                ExportSpecifier::Default(_) => {}
                ExportSpecifier::Namespace(ns) => {
                    let _name = self.get_module_export_name(&ns.name);
                }
            }
        }
        Ok(())
    }

    fn analyze_default_export(&mut self, export: &ExportDefaultDecl) -> Result<()> {
        match &export.decl {
            DefaultDecl::Class(_class) => {}
            DefaultDecl::Fn(_func) => {}
            DefaultDecl::TsInterfaceDecl(_) => {}
        }
        Ok(())
    }

    fn analyze_default_expr_export(&mut self, export: &ExportDefaultExpr) -> Result<()> {
        match &*export.expr {
            Expr::Object(obj) => {
                self.exports
                    .insert("default".to_string(), ExportedItem::Object(obj.clone()));
            }
            Expr::Array(arr) => {
                self.exports
                    .insert("default".to_string(), ExportedItem::Array(arr.clone()));
            }
            _ => {}
        }
        Ok(())
    }

    fn analyze_var_declarator(&mut self, decl: &VarDeclarator) -> Result<()> {
        if let Pat::Ident(ident) = &decl.name {
            let name = ident.id.sym.to_string();

            // Check for type annotation
            let type_annotation = self.extract_type_annotation(ident);

            if let Some(init) = &decl.init {
                match &**init {
                    Expr::Object(obj) => {
                        for prop in &obj.props {
                            if let PropOrSpread::Prop(prop) = prop
                                && let Prop::KeyValue(kv) = &**prop
                                && let PropName::Ident(_ident) = &kv.key
                            {
                            }
                        }
                        // Use type annotation to determine the type
                        match type_annotation.as_deref() {
                            Some("Block") => {
                                let block = self.extract_block_config(obj, &name)?;
                                self.exports.insert(name, ExportedItem::Block(block));
                            }
                            Some(collection_type)
                                if collection_type.starts_with("CollectionConfig") =>
                            {
                                self.extract_collection_config(obj)?;
                                // Also add the variable to exports so it can be imported
                                self.exports.insert(name, ExportedItem::Object(obj.clone()));
                            }
                            Some(field_type)
                                if field_type.ends_with("Field")
                                    || field_type == "Field[]"
                                    || field_type == "Field" =>
                            {
                                self.exports.insert(name, ExportedItem::Object(obj.clone()));
                            }
                            _ => {
                                // Fallback to property-based detection
                                if self.is_block_config(obj) {
                                    let block = self.extract_block_config(obj, &name)?;
                                    self.exports.insert(name, ExportedItem::Block(block));
                                } else if self.is_collection_config(obj) {
                                    self.extract_collection_config(obj)?;
                                    // Also add the variable to exports so it can be imported
                                    self.exports.insert(name, ExportedItem::Object(obj.clone()));
                                } else if self.is_field_config(obj) {
                                    self.extract_field_config(obj, &name)?;
                                    self.exports.insert(name, ExportedItem::Object(obj.clone()));
                                } else {
                                    self.exports.insert(name, ExportedItem::Object(obj.clone()));
                                }
                            }
                        }
                    }
                    Expr::Array(arr) => {
                        let fields = self.extract_fields_from_array(arr)?;
                        if !fields.is_empty() {
                            self.exports.insert(name, ExportedItem::Fields(fields));
                        }
                    }
                    Expr::Call(call) => {
                        self.analyze_call_expression(call, Some(name))?;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn analyze_call_expression(
        &mut self,
        call: &CallExpr,
        export_name: Option<String>,
    ) -> Result<()> {
        let func_name = self.extract_call_name(call);

        // Handle common Payload field functions
        if func_name == "slugField" {
            let slug_field = FieldDefinition {
                name: "slug".to_string(),
                field_type: FieldType::Text {
                    min_length: None,
                    max_length: None,
                },
                required: true,
                default_value: None,
                label: Some("Slug".to_string()),
                admin: Some(FieldAdmin {
                    position: Some("sidebar".to_string()),
                    description: None,
                    condition: None,
                    hidden: None,
                }),
                source_location: SourceLocation::default(),
            };

            if let Some(name) = export_name {
                self.exports
                    .insert(name, ExportedItem::Fields(vec![slug_field]));
            }
        }

        Ok(())
    }

    fn extract_type_annotation(&self, _ident: &Ident) -> Option<String> {
        // Note: In SWC, type annotations are not directly accessible from Ident
        // They are typically found in the parent context (VarDeclarator, etc.)
        // For now, we'll return None and rely on property-based detection
        // TODO: Implement proper type annotation extraction from parent context
        None
    }

    fn is_block_config(&self, obj: &ObjectLit) -> bool {
        let mut has_slug = false;
        let mut has_fields = false;
        let mut has_labels_or_interface = false;

        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop
                && let Prop::KeyValue(kv) = &**prop
                && let PropName::Ident(ident) = &kv.key
            {
                match ident.sym.as_ref() {
                    "slug" => has_slug = true,
                    "fields" => has_fields = true,
                    "labels" | "interfaceName" => has_labels_or_interface = true,
                    _ => {}
                }
            }
        }

        // Block configs have slug, fields, and either labels or interfaceName
        has_slug && has_fields && has_labels_or_interface
    }

    fn is_collection_config(&self, obj: &ObjectLit) -> bool {
        let mut has_slug = false;
        let mut has_fields = false;
        let mut has_collection_props = false;

        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop
                && let Prop::KeyValue(kv) = &**prop
                && let PropName::Ident(ident) = &kv.key
            {
                match ident.sym.as_ref() {
                    "slug" => has_slug = true,
                    "fields" => has_fields = true,
                    "hooks" | "access" | "admin" | "versions" | "endpoints" => {
                        has_collection_props = true
                    }
                    _ => {}
                }
            }
        }

        // Collection configs have slug and fields, and optionally collection-specific properties
        // If it has collection-specific properties, it's definitely a collection
        // If it only has slug and fields, it could be a collection or block, so we need to check for block properties
        if has_slug && has_fields {
            if has_collection_props {
                return true; // Definitely a collection
            } else {
                // Check if it has block-specific properties
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop
                        && let Prop::KeyValue(kv) = &**prop
                        && let PropName::Ident(ident) = &kv.key
                        && (ident.sym == "labels" || ident.sym == "interfaceName")
                    {
                        return false; // Has block properties, so it's a block
                    }
                }
                // No block properties found, so it's likely a collection
                return true;
            }
        }

        false
    }

    fn is_field_config(&self, obj: &ObjectLit) -> bool {
        let mut has_name = false;
        let mut has_type = false;

        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop
                && let Prop::KeyValue(kv) = &**prop
                && let PropName::Ident(ident) = &kv.key
            {
                match ident.sym.as_ref() {
                    "name" => has_name = true,
                    "type" => has_type = true,
                    _ => {}
                }
            }
        }

        // Field configs have name and type (like group fields, array fields, etc.)
        has_name && has_type
    }

    fn extract_field_config(&mut self, obj: &ObjectLit, name: &str) -> Result<()> {
        // Extract the field type
        let mut field_type = String::new();
        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop
                && let Prop::KeyValue(kv) = &**prop
                && let PropName::Ident(ident) = &kv.key
                && ident.sym == "type"
                && let Expr::Lit(lit) = &*kv.value
                && let Lit::Str(str_lit) = &lit
            {
                field_type = str_lit.value.to_atom_lossy().to_string();
            }
        }

        // If this is a group field, extract its nested fields
        if field_type == "group" {
            for prop in &obj.props {
                if let PropOrSpread::Prop(prop) = prop
                    && let Prop::KeyValue(kv) = &**prop
                    && let PropName::Ident(ident) = &kv.key
                    && ident.sym == "fields"
                    && let Expr::Array(arr) = &*kv.value
                {
                    let group_fields = self.extract_fields_from_array(arr)?;
                    // Add the group field itself
                    self.fields.push(FieldDefinition {
                        name: name.to_string(),
                        field_type: FieldType::Group { fields: Vec::new() },
                        required: false, // Default, could be extracted from properties
                        default_value: None,
                        label: None,
                        admin: None,
                        source_location: SourceLocation::default(),
                    });
                    // Add the nested fields with the group name as prefix
                    for mut field in group_fields {
                        field.name = format!("{}.{}", name, field.name);
                        self.fields.push(field);
                    }
                }
            }
        } else {
            // For non-group fields, just add the field definition
            let field_type_enum = match field_type.as_str() {
                "text" => FieldType::Text {
                    min_length: None,
                    max_length: None,
                },
                "number" => FieldType::Number {
                    min: None,
                    max: None,
                },
                "date" => FieldType::Date { admin: None },
                "checkbox" => FieldType::Checkbox,
                "select" => FieldType::Select {
                    options: Vec::new(),
                },
                "relationship" => FieldType::Relationship {
                    relationTo: String::new(),
                },
                "array" => FieldType::Array { fields: Vec::new() },
                "blocks" => FieldType::Blocks { blocks: Vec::new() },
                "upload" => FieldType::Upload {
                    relationTo: String::new(),
                },
                "richText" => FieldType::RichText,
                "json" => FieldType::Json,
                "point" => FieldType::Point,
                _ => FieldType::Text {
                    min_length: None,
                    max_length: None,
                }, // Default fallback
            };
            self.fields.push(FieldDefinition {
                name: name.to_string(),
                field_type: field_type_enum,
                required: false, // Default, could be extracted from properties
                default_value: None,
                label: None,
                admin: None,
                source_location: SourceLocation::default(),
            });
        }

        Ok(())
    }

    fn extract_collection_config(&mut self, obj: &ObjectLit) -> Result<()> {
        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop
                && let Prop::KeyValue(kv) = &**prop
                && let PropName::Ident(ident) = &kv.key
            {
                match ident.sym.as_ref() {
                    "fields" => {
                        if let Expr::Array(arr) = &*kv.value {
                            let mut new_fields = self.extract_fields_from_array(arr)?;
                            self.fields.append(&mut new_fields);
                        }
                    }
                    "slug" => {}
                    "hooks" => {}
                    "access" => {}
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn extract_block_config(&self, obj: &ObjectLit, name: &str) -> Result<BlockDefinition> {
        let mut slug = name.to_lowercase();
        let mut fields = Vec::new();
        let mut labels = BlockLabels {
            singular: None,
            plural: None,
        };

        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop
                && let Prop::KeyValue(kv) = &**prop
                && let PropName::Ident(ident) = &kv.key
            {
                match ident.sym.as_ref() {
                    "slug" => {
                        if let Expr::Lit(lit) = &*kv.value
                            && let Lit::Str(str_lit) = &lit
                        {
                            slug = str_lit.value.to_atom_lossy().to_string();
                        }
                    }
                    "fields" => {
                        if let Expr::Array(arr) = &*kv.value {
                            fields = self.extract_fields_from_array(arr)?;
                        }
                    }
                    "labels" => {
                        if let Expr::Object(labels_obj) = &*kv.value {
                            for label_prop in &labels_obj.props {
                                if let PropOrSpread::Prop(label_prop) = label_prop
                                    && let Prop::KeyValue(label_kv) = &**label_prop
                                    && let PropName::Ident(label_ident) = &label_kv.key
                                    && let Expr::Lit(label_lit) = &*label_kv.value
                                    && let Lit::Str(label_str) = &label_lit
                                {
                                    match label_ident.sym.as_ref() {
                                        "singular" => {
                                            labels.singular =
                                                Some(label_str.value.to_atom_lossy().to_string())
                                        }
                                        "plural" => {
                                            labels.plural =
                                                Some(label_str.value.to_atom_lossy().to_string())
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(BlockDefinition {
            slug,
            fields,
            labels,
        })
    }

    fn extract_fields_from_array(&self, arr: &ArrayLit) -> Result<Vec<FieldDefinition>> {
        let mut fields = Vec::new();

        for elem in arr.elems.iter().flatten() {
            {
                if elem.spread.is_some() {
                    // Handle spread element
                    match &*elem.expr {
                        Expr::Call(call) => {
                            let func_name = self.extract_call_name(call);
                            fields.push(FieldDefinition {
                                name: format!("__spread__call__{}", func_name),
                                field_type: FieldType::Text {
                                    min_length: None,
                                    max_length: None,
                                },
                                required: false,
                                default_value: None,
                                label: None,
                                admin: None,
                                source_location: SourceLocation::default(),
                            });
                        }
                        Expr::Ident(ident) => {
                            fields.push(FieldDefinition {
                                name: format!("__spread__{}", ident.sym),
                                field_type: FieldType::Text {
                                    min_length: None,
                                    max_length: None,
                                },
                                required: false,
                                default_value: None,
                                label: None,
                                admin: None,
                                source_location: SourceLocation::default(),
                            });
                        }
                        _ => {}
                    }
                } else {
                    // Regular element
                    match &*elem.expr {
                        Expr::Object(obj) => {
                            if let Some(field) = self.extract_field_from_object(obj)? {
                                fields.push(field);
                            }
                        }
                        Expr::Ident(ident) => {
                            fields.push(FieldDefinition {
                                name: format!("__ref__{}", ident.sym),
                                field_type: FieldType::Text {
                                    min_length: None,
                                    max_length: None,
                                },
                                required: false,
                                default_value: None,
                                label: None,
                                admin: None,
                                source_location: SourceLocation::default(),
                            });
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(fields)
    }

    fn extract_field_from_object(&self, obj: &ObjectLit) -> Result<Option<FieldDefinition>> {
        let mut field = FieldDefinition {
            name: String::new(),
            field_type: FieldType::Text {
                min_length: None,
                max_length: None,
            },
            required: false,
            default_value: None,
            label: None,
            admin: None,
            source_location: SourceLocation::default(),
        };

        let mut has_name = false;

        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop
                && let Prop::KeyValue(kv) = &**prop
                && let PropName::Ident(key) = &kv.key
            {
                match key.sym.as_ref() {
                    "name" => {
                        if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                            field.name = s.value.to_atom_lossy().to_string();
                            has_name = true;
                        }
                    }
                    "type" => {
                        if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                            let type_str = s.value.to_atom_lossy().to_string();
                            field.field_type = self.parse_field_type(&type_str, obj)?;
                        }
                    }
                    "required" => {
                        if let Expr::Lit(Lit::Bool(b)) = &*kv.value {
                            field.required = b.value;
                        }
                    }
                    "label" => {
                        if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                            field.label = Some(s.value.to_atom_lossy().to_string());
                        }
                    }
                    "defaultValue" => {
                        field.default_value = self.extract_json_value(&kv.value);
                    }
                    "admin" => {
                        if let Expr::Object(admin_obj) = &*kv.value {
                            field.admin = Some(self.extract_field_admin(admin_obj)?);
                        }
                    }
                    _ => {}
                }
            }
        }

        if has_name { Ok(Some(field)) } else { Ok(None) }
    }

    fn extract_field_admin(&self, obj: &ObjectLit) -> Result<FieldAdmin> {
        let mut admin = FieldAdmin {
            position: None,
            description: None,
            condition: None,
            hidden: None,
        };

        for prop in &obj.props {
            if let PropOrSpread::Prop(prop) = prop
                && let Prop::KeyValue(kv) = &**prop
                && let PropName::Ident(key) = &kv.key
            {
                match key.sym.as_ref() {
                    "position" => {
                        if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                            admin.position = Some(s.value.to_atom_lossy().to_string());
                        }
                    }
                    "description" => {
                        if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                            admin.description = Some(s.value.to_atom_lossy().to_string());
                        }
                    }
                    "hidden" => {
                        if let Expr::Lit(Lit::Bool(b)) = &*kv.value {
                            admin.hidden = Some(b.value);
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(admin)
    }

    fn parse_field_type(&self, type_str: &str, obj: &ObjectLit) -> Result<FieldType> {
        let field_type = match type_str {
            "text" => {
                let mut min_length = None;
                let mut max_length = None;

                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop
                        && let Prop::KeyValue(kv) = &**prop
                        && let PropName::Ident(key) = &kv.key
                    {
                        match key.sym.as_ref() {
                            "minLength" => {
                                if let Expr::Lit(Lit::Num(n)) = &*kv.value {
                                    min_length = Some(n.value as u32);
                                }
                            }
                            "maxLength" => {
                                if let Expr::Lit(Lit::Num(n)) = &*kv.value {
                                    max_length = Some(n.value as u32);
                                }
                            }
                            _ => {}
                        }
                    }
                }

                FieldType::Text {
                    min_length,
                    max_length,
                }
            }
            "number" => FieldType::Number {
                min: None,
                max: None,
            },
            "checkbox" => FieldType::Checkbox,
            "date" => FieldType::Date { admin: None },
            "select" => {
                let mut options = Vec::new();
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop
                        && let Prop::KeyValue(kv) = &**prop
                        && let PropName::Ident(key) = &kv.key
                        && key.sym == "options"
                        && let Expr::Array(arr) = &*kv.value
                    {
                        options = self.extract_select_options(arr)?;
                    }
                }
                FieldType::Select { options }
            }
            "relationship" => {
                let mut relation_to = String::new();
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop
                        && let Prop::KeyValue(kv) = &**prop
                        && let PropName::Ident(key) = &kv.key
                        && key.sym == "relationTo"
                        && let Expr::Lit(Lit::Str(s)) = &*kv.value
                    {
                        relation_to = s.value.to_atom_lossy().to_string();
                    }
                }
                FieldType::Relationship {
                    relationTo: relation_to,
                }
            }
            "array" => {
                let mut nested_fields = Vec::new();
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop
                        && let Prop::KeyValue(kv) = &**prop
                        && let PropName::Ident(key) = &kv.key
                        && key.sym == "fields"
                        && let Expr::Array(arr) = &*kv.value
                    {
                        nested_fields = self.extract_fields_from_array(arr)?;
                    }
                }
                FieldType::Array {
                    fields: nested_fields,
                }
            }
            "tabs" => {
                let mut tabs = Vec::new();
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop
                        && let Prop::KeyValue(kv) = &**prop
                        && let PropName::Ident(key) = &kv.key
                        && key.sym == "tabs"
                        && let Expr::Array(arr) = &*kv.value
                    {
                        tabs = self.extract_tabs_from_array(arr)?;
                    }
                }
                FieldType::Tabs { tabs }
            }
            "blocks" => {
                let mut blocks = Vec::new();
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop
                        && let Prop::KeyValue(kv) = &**prop
                        && let PropName::Ident(key) = &kv.key
                        && key.sym == "blocks"
                        && let Expr::Array(arr) = &*kv.value
                    {
                        blocks = self.extract_block_names(arr)?;
                    }
                }
                FieldType::Blocks { blocks }
            }
            "richText" => FieldType::RichText,
            "json" => FieldType::Json,
            "point" => FieldType::Point,
            _ => FieldType::Text {
                min_length: None,
                max_length: None,
            },
        };

        Ok(field_type)
    }

    fn extract_select_options(&self, arr: &ArrayLit) -> Result<Vec<SelectOption>> {
        let mut options = Vec::new();

        for elem in arr.elems.iter().flatten() {
            match &*elem.expr {
                Expr::Object(obj) => {
                    let mut label = String::new();
                    let mut value = String::new();

                    for prop in &obj.props {
                        if let PropOrSpread::Prop(prop) = prop
                            && let Prop::KeyValue(kv) = &**prop
                            && let PropName::Ident(key) = &kv.key
                        {
                            match key.sym.as_ref() {
                                "label" => {
                                    if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                        label = s.value.to_atom_lossy().to_string();
                                    }
                                }
                                "value" => {
                                    if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                        value = s.value.to_atom_lossy().to_string();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }

                    if !label.is_empty() && !value.is_empty() {
                        options.push(SelectOption { label, value });
                    }
                }
                Expr::Lit(Lit::Str(s)) => {
                    // Simple string option
                    let value = s.value.to_atom_lossy().to_string();
                    options.push(SelectOption {
                        label: value.clone(),
                        value,
                    });
                }
                _ => {}
            }
        }

        Ok(options)
    }

    fn extract_block_names(&self, arr: &ArrayLit) -> Result<Vec<String>> {
        let mut blocks = Vec::new();

        for elem in arr.elems.iter().flatten() {
            if let Expr::Ident(ident) = &*elem.expr {
                blocks.push(ident.sym.to_string());
            }
        }

        Ok(blocks)
    }

    fn extract_tabs_from_array(&self, arr: &ArrayLit) -> Result<Vec<TabDefinition>> {
        let mut tabs = Vec::new();

        for elem in arr.elems.iter().flatten() {
            if let Expr::Object(obj) = &*elem.expr {
                let mut tab = TabDefinition {
                    label: String::new(),
                    fields: Vec::new(),
                    description: None,
                };

                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop
                        && let Prop::KeyValue(kv) = &**prop
                        && let PropName::Ident(key) = &kv.key
                    {
                        match key.sym.as_ref() {
                            "label" => {
                                if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                    tab.label = s.value.to_atom_lossy().to_string();
                                }
                            }
                            "fields" => {
                                if let Expr::Array(fields_arr) = &*kv.value {
                                    tab.fields = self.extract_fields_from_array(fields_arr)?;
                                }
                            }
                            "description" => {
                                if let Expr::Lit(Lit::Str(s)) = &*kv.value {
                                    tab.description = Some(s.value.to_atom_lossy().to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                }

                tabs.push(tab);
            }
        }

        Ok(tabs)
    }

    fn extract_json_value(&self, expr: &Expr) -> Option<serde_json::Value> {
        match expr {
            Expr::Lit(lit) => match lit {
                Lit::Str(s) => Some(serde_json::Value::String(
                    s.value.to_atom_lossy().to_string(),
                )),
                Lit::Bool(b) => Some(serde_json::Value::Bool(b.value)),
                Lit::Num(n) => Some(serde_json::Value::Number(
                    serde_json::Number::from_f64(n.value)
                        .unwrap_or_else(|| serde_json::Number::from(0)),
                )),
                Lit::Null(_) => Some(serde_json::Value::Null),
                _ => None,
            },
            Expr::Array(arr) => {
                let values: Vec<serde_json::Value> = arr
                    .elems
                    .iter()
                    .filter_map(|elem| elem.as_ref())
                    .filter_map(|elem| self.extract_json_value(&elem.expr))
                    .collect();
                Some(serde_json::Value::Array(values))
            }
            Expr::Object(obj) => {
                let mut map = serde_json::Map::new();
                for prop in &obj.props {
                    if let PropOrSpread::Prop(prop) = prop
                        && let Prop::KeyValue(kv) = &**prop
                        && let PropName::Ident(key) = &kv.key
                        && let Some(value) = self.extract_json_value(&kv.value)
                    {
                        map.insert(key.sym.to_string(), value);
                    }
                }
                Some(serde_json::Value::Object(map))
            }
            _ => None,
        }
    }

    fn extract_call_name(&self, call: &CallExpr) -> String {
        match &call.callee {
            Callee::Expr(expr) => match &**expr {
                Expr::Ident(ident) => ident.sym.to_string(),
                _ => "unknown".to_string(),
            },
            _ => "unknown".to_string(),
        }
    }

    fn get_module_export_name(&self, export: &ModuleExportName) -> String {
        match export {
            ModuleExportName::Ident(ident) => ident.sym.to_string(),
            ModuleExportName::Str(s) => s.value.to_atom_lossy().to_string(),
        }
    }
}
