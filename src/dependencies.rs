use crate::lexer::is_ident_start;
use crate::lexer::is_white_space;
use crate::lexer::C_ASTERISK;
use crate::lexer::C_COLON;
use crate::lexer::C_HYPHEN_MINUS;
use crate::lexer::C_LEFT_CURLY;
use crate::lexer::C_RIGHT_CURLY;
use crate::lexer::C_RIGHT_PARENTHESIS;
use crate::lexer::C_SEMICOLON;
use crate::lexer::C_SOLIDUS;
use crate::Lexer;
use crate::Pos;
use crate::Visitor;

#[derive(Debug)]
enum Scope<'s> {
    TopLevel,
    InBlock,
    InAtImport(ImportData<'s>),
    AtImportInvalid,
    AtNamespaceInvalid,
}

#[derive(Debug)]
struct ImportData<'s> {
    start: Pos,
    url: Option<&'s str>,
    url_range: Option<Range>,
    supports: ImportDataSupports<'s>,
    layer: ImportDataLayer<'s>,
}

impl ImportData<'_> {
    pub fn new(start: Pos) -> Self {
        Self {
            start,
            url: None,
            url_range: None,
            supports: ImportDataSupports::None,
            layer: ImportDataLayer::None,
        }
    }

    pub fn in_supports(&self) -> bool {
        matches!(self.supports, ImportDataSupports::InSupports { .. })
    }

    pub fn layer_range(&self) -> Option<&Range> {
        let ImportDataLayer::EndLayer { range, .. } = &self.layer else {
            return None;
        };
        Some(range)
    }

    pub fn supports_range(&self) -> Option<&Range> {
        let ImportDataSupports::EndSupports { range, .. } = &self.supports else {
            return None;
        };
        Some(range)
    }
}

#[derive(Debug)]
enum ImportDataSupports<'s> {
    None,
    InSupports { start: Pos },
    EndSupports { value: &'s str, range: Range },
}

#[derive(Debug)]
enum ImportDataLayer<'s> {
    None,
    EndLayer { value: &'s str, range: Range },
}

#[derive(Debug)]
struct BalancedItem {
    kind: BalancedItemKind,
    range: Range,
}

impl BalancedItem {
    pub fn new(name: &str, start: Pos, end: Pos) -> Self {
        Self {
            kind: BalancedItemKind::new(name),
            range: Range::new(start, end),
        }
    }

    pub fn new_other(start: Pos, end: Pos) -> Self {
        Self {
            kind: BalancedItemKind::Other,
            range: Range::new(start, end),
        }
    }
}

#[derive(Debug)]
enum BalancedItemKind {
    Url,
    ImageSet,
    Layer,
    Supports,
    Local,
    Global,
    Other,
}

impl BalancedItemKind {
    pub fn new(name: &str) -> Self {
        match name {
            "url" => Self::Url,
            "image-set" => Self::ImageSet,
            "layer" => Self::Layer,
            "supports" => Self::Supports,
            ":local" => Self::Local,
            ":global" => Self::Global,
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Range {
    pub start: Pos,
    pub end: Pos,
}

impl Range {
    pub fn new(start: Pos, end: Pos) -> Self {
        Self { start, end }
    }
}

#[derive(Debug)]
enum CssModulesMode {
    Local,
    Global,
    None,
}

#[derive(Debug)]
pub struct CssModulesModeData {
    default: CssModulesMode,
    current: CssModulesMode,
}

impl CssModulesModeData {
    pub fn new(local: bool) -> Self {
        Self {
            default: if local {
                CssModulesMode::Local
            } else {
                CssModulesMode::Global
            },
            current: CssModulesMode::None,
        }
    }

    pub fn is_local_mode(&self) -> bool {
        match self.current {
            CssModulesMode::Local => true,
            CssModulesMode::Global => false,
            CssModulesMode::None => match self.default {
                CssModulesMode::Local => true,
                CssModulesMode::Global => false,
                CssModulesMode::None => false,
            },
        }
    }

    pub fn set_local(&mut self) {
        self.current = CssModulesMode::Local;
    }

    pub fn set_global(&mut self) {
        self.current = CssModulesMode::Global;
    }

    pub fn set_none(&mut self) {
        self.current = CssModulesMode::None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Dependency<'s> {
    Url {
        request: &'s str,
        range: Range,
        kind: UrlRangeKind,
    },
    Import {
        request: &'s str,
        range: Range,
        layer: Option<&'s str>,
        supports: Option<&'s str>,
        media: Option<&'s str>,
    },
    Replace {
        content: &'s str,
        range: Range,
    },
    LocalIdent {
        name: &'s str,
        range: Range,
    },
    LocalVar {
        name: &'s str,
        range: Range,
    },
    LocalVarDecl {
        name_range: Range,
        name: &'s str,
        value: &'s str,
    },
    ICSSExport {
        prop: &'s str,
        value: &'s str,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UrlRangeKind {
    Function,
    String,
}

#[derive(Debug, Clone)]
pub enum Warning {
    Unexpected { unexpected: Range, range: Range },
    DuplicateUrl { range: Range },
    NamespaceNotSupportedInBundledCss { range: Range },
    NotPrecededAtImport { range: Range },
    ExpectedUrl { range: Range },
    ExpectedBefore { should_after: Range, range: Range },
}

#[derive(Debug)]
pub struct LexDependencies<'s, D, W> {
    mode_data: Option<CssModulesModeData>,
    scope: Scope<'s>,
    block_nesting_level: u32,
    allow_import_at_rule: bool,
    balanced: Vec<BalancedItem>,
    is_next_rule_prelude: bool,
    handle_dependency: D,
    handle_warning: W,
}

impl<'s, D: FnMut(Dependency<'s>), W: FnMut(Warning)> LexDependencies<'s, D, W> {
    pub fn new(
        handle_dependency: D,
        handle_warning: W,
        mode_data: Option<CssModulesModeData>,
    ) -> Self {
        Self {
            mode_data,
            scope: Scope::TopLevel,
            block_nesting_level: 0,
            allow_import_at_rule: true,
            balanced: Vec::new(),
            is_next_rule_prelude: true,
            handle_dependency,
            handle_warning,
        }
    }

    fn _is_next_nested_syntax(&self, lexer: &Lexer) -> Option<bool> {
        let mut lexer = lexer.clone();
        lexer.consume_white_space_and_comments()?;
        let c = lexer.cur()?;
        if c == C_LEFT_CURLY {
            return Some(false);
        }
        Some(!is_ident_start(c))
    }

    fn get_media(&self, lexer: &Lexer<'s>, start: Pos, end: Pos) -> Option<&'s str> {
        let media = lexer.slice(start, end)?;
        let mut media_lexer = Lexer::from(media);
        media_lexer.consume()?;
        media_lexer.consume_white_space_and_comments()?;
        Some(media)
    }

    fn consume_icss_export_prop(&self, lexer: &mut Lexer<'s>) -> Option<()> {
        loop {
            let c = lexer.cur()?;
            if c == C_COLON
                || c == C_RIGHT_CURLY
                || c == C_SEMICOLON
                || (c == C_SOLIDUS && lexer.peek()? == C_ASTERISK)
            {
                break;
            }
            lexer.consume()?;
        }
        Some(())
    }

    fn consume_icss_export_value(&self, lexer: &mut Lexer<'s>) -> Option<()> {
        loop {
            let c = lexer.cur()?;
            if c == C_RIGHT_CURLY || c == C_SEMICOLON {
                break;
            }
            lexer.consume()?;
        }
        Some(())
    }

    fn lex_icss_export(&mut self, lexer: &mut Lexer<'s>, start: Pos) -> Option<()> {
        lexer.consume_white_space_and_comments()?;
        let c = lexer.cur()?;
        if c != C_LEFT_CURLY {
            let end = lexer.peek_pos()?;
            (self.handle_warning)(Warning::Unexpected {
                unexpected: Range::new(lexer.cur_pos()?, end),
                range: Range::new(start, end),
            });
            return Some(());
        }
        lexer.consume()?;
        lexer.consume_white_space_and_comments()?;
        while lexer.cur()? != C_RIGHT_CURLY {
            lexer.consume_white_space_and_comments()?;
            let prop_start = lexer.cur_pos()?;
            self.consume_icss_export_prop(lexer)?;
            let prop_end = lexer.cur_pos()?;
            lexer.consume_white_space_and_comments()?;
            if lexer.cur()? != C_COLON {
                let end = lexer.peek_pos()?;
                (self.handle_warning)(Warning::Unexpected {
                    unexpected: Range::new(lexer.cur_pos()?, end),
                    range: Range::new(prop_start, end),
                });
                return Some(());
            }
            lexer.consume()?;
            lexer.consume_white_space_and_comments()?;
            let value_start = lexer.cur_pos()?;
            self.consume_icss_export_value(lexer)?;
            let value_end = lexer.cur_pos()?;
            if lexer.cur()? == C_SEMICOLON {
                lexer.consume()?;
                lexer.consume_white_space_and_comments()?;
            }
            (self.handle_dependency)(Dependency::ICSSExport {
                prop: lexer
                    .slice(prop_start, prop_end)?
                    .trim_end_matches(is_white_space),
                value: lexer
                    .slice(value_start, value_end)?
                    .trim_end_matches(is_white_space),
            });
        }
        lexer.consume()?;
        Some(())
    }

    fn lex_local_var(&mut self, lexer: &mut Lexer<'s>, start: Pos) -> Option<()> {
        lexer.consume_white_space_and_comments()?;
        let minus_start = lexer.cur_pos()?;
        if lexer.cur()? != C_HYPHEN_MINUS || lexer.peek()? != C_HYPHEN_MINUS {
            let end = lexer.peek2_pos()?;
            (self.handle_warning)(Warning::Unexpected {
                unexpected: Range::new(minus_start, end),
                range: Range::new(start, end),
            });
            return Some(());
        }
        lexer.consume_ident_sequence()?;
        let start = minus_start + 2;
        let end = lexer.cur_pos()?;
        lexer.consume_white_space_and_comments()?;
        if lexer.cur()? != C_RIGHT_PARENTHESIS {
            let end = lexer.peek_pos()?;
            (self.handle_warning)(Warning::Unexpected {
                unexpected: Range::new(lexer.cur_pos()?, end),
                range: Range::new(start, end),
            });
            return Some(());
        }
        (self.handle_dependency)(Dependency::LocalVar {
            name: lexer.slice(start, end)?,
            range: Range::new(minus_start, end),
        });
        Some(())
    }

    fn lex_local_var_decl(
        &mut self,
        lexer: &mut Lexer<'s>,
        name: &'s str,
        start: Pos,
        end: Pos,
    ) -> Option<()> {
        lexer.consume_white_space_and_comments()?;
        if lexer.cur()? != C_COLON {
            let end = lexer.peek_pos()?;
            (self.handle_warning)(Warning::Unexpected {
                unexpected: Range::new(lexer.cur_pos()?, end),
                range: Range::new(start, end),
            });
            return Some(());
        }
        lexer.consume()?;
        lexer.consume_white_space_and_comments()?;
        let value_start = lexer.cur_pos()?;
        self.consume_icss_export_value(lexer)?;
        let value_end = lexer.cur_pos()?;
        if lexer.cur()? == C_SEMICOLON {
            lexer.consume()?;
            lexer.consume_white_space_and_comments()?;
        }
        (self.handle_dependency)(Dependency::LocalVarDecl {
            name_range: Range::new(start, end),
            name,
            value: lexer.slice(value_start, value_end)?,
        });
        Some(())
    }
}

impl<'s, D: FnMut(Dependency<'s>), W: FnMut(Warning)> Visitor<'s> for LexDependencies<'s, D, W> {
    fn is_selector(&mut self, _: &mut Lexer) -> Option<bool> {
        Some(self.is_next_rule_prelude)
    }

    fn url(
        &mut self,
        lexer: &mut Lexer<'s>,
        start: Pos,
        end: Pos,
        content_start: Pos,
        content_end: Pos,
    ) -> Option<()> {
        let value = lexer.slice(content_start, content_end)?;
        match self.scope {
            Scope::InAtImport(ref mut import_data) => {
                if import_data.in_supports() {
                    return Some(());
                }
                if import_data.url.is_some() {
                    (self.handle_warning)(Warning::DuplicateUrl {
                        range: Range::new(import_data.start, end),
                    });
                    return Some(());
                }
                import_data.url = Some(value);
                import_data.url_range = Some(Range::new(start, end));
            }
            Scope::InBlock => (self.handle_dependency)(Dependency::Url {
                request: value,
                range: Range::new(start, end),
                kind: UrlRangeKind::Function,
            }),
            _ => {}
        }
        Some(())
    }

    fn string(&mut self, lexer: &mut Lexer<'s>, start: Pos, end: Pos) -> Option<()> {
        match self.scope {
            Scope::InAtImport(ref mut import_data) => {
                let inside_url = matches!(
                    self.balanced.last(),
                    Some(last) if matches!(last.kind, BalancedItemKind::Url)
                );

                // Do not parse URLs in `supports(...)` and other strings if we already have a URL
                if import_data.in_supports() || (!inside_url && import_data.url.is_some()) {
                    return Some(());
                }

                if inside_url && import_data.url.is_some() {
                    (self.handle_warning)(Warning::DuplicateUrl {
                        range: Range::new(import_data.start, end),
                    });
                    return Some(());
                }

                let value = lexer.slice(start + 1, end - 1)?;
                import_data.url = Some(value);
                // For url("inside_url") url_range will determined in right_parenthesis
                if !inside_url {
                    import_data.url_range = Some(Range::new(start, end));
                }
            }
            Scope::InBlock => {
                let Some(last) = self.balanced.last() else {
                    return Some(());
                };
                let kind = match last.kind {
                    BalancedItemKind::Url => UrlRangeKind::String,
                    BalancedItemKind::ImageSet => UrlRangeKind::Function,
                    _ => return Some(()),
                };
                let value = lexer.slice(start + 1, end - 1)?;
                (self.handle_dependency)(Dependency::Url {
                    request: value,
                    range: Range::new(start, end),
                    kind,
                });
            }
            _ => {}
        }
        Some(())
    }

    fn at_keyword(&mut self, lexer: &mut Lexer, start: Pos, end: Pos) -> Option<()> {
        let name = lexer.slice(start, end)?.to_ascii_lowercase();
        if name == "@namespace" {
            self.scope = Scope::AtNamespaceInvalid;
            (self.handle_warning)(Warning::NamespaceNotSupportedInBundledCss {
                range: Range::new(start, end),
            });
        } else if name == "@import" {
            if !self.allow_import_at_rule {
                self.scope = Scope::AtImportInvalid;
                (self.handle_warning)(Warning::NotPrecededAtImport {
                    range: Range::new(start, end),
                });
                return Some(());
            }
            self.scope = Scope::InAtImport(ImportData::new(start));
        } else if name == "@media"
            || name == "@supports"
            || name == "@layer"
            || name == "@container"
        {
            self.is_next_rule_prelude = true;
        }
        // else if self.allow_mode_switch {
        //     self.is_next_rule_prelude = false;
        // }
        Some(())
    }

    fn semicolon(&mut self, lexer: &mut Lexer<'s>, start: Pos, end: Pos) -> Option<()> {
        match self.scope {
            Scope::InAtImport(ref import_data) => {
                let Some(url) = import_data.url else {
                    (self.handle_warning)(Warning::ExpectedUrl {
                        range: Range::new(import_data.start, end),
                    });
                    self.scope = Scope::TopLevel;
                    return Some(());
                };
                let Some(url_range) = &import_data.url_range else {
                    (self.handle_warning)(Warning::Unexpected {
                        unexpected: Range::new(start, end),
                        range: Range::new(import_data.start, end),
                    });
                    self.scope = Scope::TopLevel;
                    return Some(());
                };
                let layer = match &import_data.layer {
                    ImportDataLayer::None => None,
                    ImportDataLayer::EndLayer { value, range } => {
                        if url_range.start > range.start {
                            (self.handle_warning)(Warning::ExpectedBefore {
                                should_after: range.clone(),
                                range: url_range.clone(),
                            });
                            self.scope = Scope::TopLevel;
                            return Some(());
                        }
                        Some(*value)
                    }
                };
                let supports = match &import_data.supports {
                    ImportDataSupports::None => None,
                    ImportDataSupports::InSupports {
                        start: supports_start,
                    } => {
                        (self.handle_warning)(Warning::Unexpected {
                            unexpected: Range::new(start, end),
                            range: Range::new(*supports_start, end),
                        });
                        None
                    }
                    ImportDataSupports::EndSupports { value, range } => {
                        if url_range.start > range.start {
                            (self.handle_warning)(Warning::ExpectedBefore {
                                should_after: range.clone(),
                                range: url_range.clone(),
                            });
                            self.scope = Scope::TopLevel;
                            return Some(());
                        }
                        Some(*value)
                    }
                };
                if let Some(layer_range) = import_data.layer_range() {
                    if let Some(supports_range) = import_data.supports_range() {
                        if layer_range.start > supports_range.start {
                            (self.handle_warning)(Warning::ExpectedBefore {
                                should_after: supports_range.clone(),
                                range: layer_range.clone(),
                            });
                            self.scope = Scope::TopLevel;
                            return Some(());
                        }
                    }
                }
                let last_end = import_data
                    .supports_range()
                    .or_else(|| import_data.layer_range())
                    .unwrap_or(url_range)
                    .end;
                let media = self.get_media(lexer, last_end, start);
                (self.handle_dependency)(Dependency::Import {
                    request: url,
                    range: Range::new(import_data.start, end),
                    layer,
                    supports,
                    media,
                });
                self.scope = Scope::TopLevel;
            }
            Scope::AtImportInvalid | Scope::AtNamespaceInvalid => {
                self.scope = Scope::TopLevel;
            }
            Scope::InBlock => {
                // TODO: css modules
            }
            _ => {}
        }
        Some(())
    }

    fn function(&mut self, lexer: &mut Lexer<'s>, start: Pos, end: Pos) -> Option<()> {
        let name = lexer.slice(start, end - 1)?.to_ascii_lowercase();
        self.balanced.push(BalancedItem::new(&name, start, end));

        if let Scope::InAtImport(ref mut import_data) = self.scope {
            if name == "supports" {
                import_data.supports = ImportDataSupports::InSupports { start };
            }
        }

        let Some(mode_data) = &self.mode_data else {
            return Some(());
        };
        if mode_data.is_local_mode() && name == "var" {
            self.lex_local_var(lexer, start)?;
        }
        Some(())
    }

    fn left_parenthesis(&mut self, _: &mut Lexer, start: Pos, end: Pos) -> Option<()> {
        self.balanced.push(BalancedItem::new_other(start, end));
        Some(())
    }

    fn right_parenthesis(&mut self, lexer: &mut Lexer<'s>, start: Pos, end: Pos) -> Option<()> {
        let Some(last) = self.balanced.pop() else {
            return Some(());
        };
        if let Some(mode_data) = &mut self.mode_data {
            if matches!(
                last.kind,
                BalancedItemKind::Local | BalancedItemKind::Global
            ) {
                match self.balanced.last() {
                    Some(last) if matches!(last.kind, BalancedItemKind::Local) => {
                        mode_data.set_local()
                    }
                    Some(last) if matches!(last.kind, BalancedItemKind::Global) => {
                        mode_data.set_global()
                    }
                    _ => mode_data.set_none(),
                };
                (self.handle_dependency)(Dependency::Replace {
                    content: "",
                    range: Range::new(start, end),
                });
                return Some(());
            }
        }
        if let Scope::InAtImport(ref mut import_data) = self.scope {
            let not_in_supports = !import_data.in_supports();
            if matches!(last.kind, BalancedItemKind::Url) && not_in_supports {
                import_data.url_range = Some(Range::new(last.range.start, end));
            } else if matches!(last.kind, BalancedItemKind::Layer) && not_in_supports {
                import_data.layer = ImportDataLayer::EndLayer {
                    value: lexer.slice(last.range.end, end - 1)?,
                    range: Range::new(last.range.start, end),
                };
            } else if matches!(last.kind, BalancedItemKind::Supports) {
                import_data.supports = ImportDataSupports::EndSupports {
                    value: lexer.slice(last.range.end, end - 1)?,
                    range: Range::new(last.range.start, end),
                }
            }
        }
        Some(())
    }

    fn ident(&mut self, lexer: &mut Lexer<'s>, start: Pos, end: Pos) -> Option<()> {
        match self.scope {
            Scope::InBlock => {
                let Some(mode_data) = &mut self.mode_data else {
                    return Some(());
                };
                if mode_data.is_local_mode() {
                    if let Some(name) = lexer.slice(start, end)?.strip_prefix("--") {
                        self.lex_local_var_decl(lexer, name, start, end)?;
                    }
                }
            }
            Scope::InAtImport(ref mut import_data) => {
                if lexer.slice(start, end)?.to_ascii_lowercase() == "layer" {
                    import_data.layer = ImportDataLayer::EndLayer {
                        value: "",
                        range: Range::new(start, end),
                    }
                }
            }
            _ => {}
        }
        Some(())
    }

    fn class(&mut self, lexer: &mut Lexer<'s>, start: Pos, end: Pos) -> Option<()> {
        let Some(mode_data) = &self.mode_data else {
            return Some(());
        };
        if mode_data.is_local_mode() {
            let start = start + 1;
            let name = lexer.slice(start, end)?;
            (self.handle_dependency)(Dependency::LocalIdent {
                name,
                range: Range::new(start, end),
            })
        }
        Some(())
    }

    fn id(&mut self, lexer: &mut Lexer<'s>, start: Pos, end: Pos) -> Option<()> {
        let Some(mode_data) = &self.mode_data else {
            return Some(());
        };
        if mode_data.is_local_mode() {
            let start = start + 1;
            let name = lexer.slice(start, end)?;
            (self.handle_dependency)(Dependency::LocalIdent {
                name,
                range: Range::new(start, end),
            })
        }
        Some(())
    }

    fn left_curly_bracket(&mut self, _: &mut Lexer, _: Pos, _: Pos) -> Option<()> {
        match self.scope {
            Scope::TopLevel => {
                self.allow_import_at_rule = false;
                self.scope = Scope::InBlock;
                self.block_nesting_level = 1;
                // if self.allow_mode_switch {
                //     self.is_next_rule_prelude = self.is_next_nested_syntax(lexer)?;
                // }
            }
            Scope::InBlock => {
                self.block_nesting_level += 1;
                // if self.allow_mode_switch {
                //     self.is_next_rule_prelude = self.is_next_nested_syntax(lexer)?;
                // }
            }
            _ => {}
        }
        Some(())
    }

    fn right_curly_bracket(&mut self, _: &mut Lexer, _: Pos, _: Pos) -> Option<()> {
        if matches!(self.scope, Scope::InBlock) {
            self.block_nesting_level -= 1;
            if self.block_nesting_level == 0 {
                // TODO: if isLocalMode
                self.scope = Scope::TopLevel;
                // if self.allow_mode_switch {
                //     self.is_next_rule_prelude = true;
                // }
            }
            // else if self.allow_mode_switch {
            //     self.is_next_rule_prelude = self.is_next_nested_syntax(lexer)?;
            // }
        }
        Some(())
    }

    fn pseudo_function(&mut self, lexer: &mut Lexer, start: Pos, end: Pos) -> Option<()> {
        let name = lexer.slice(start, end - 1)?.to_ascii_lowercase();
        self.balanced.push(BalancedItem::new(&name, start, end));
        if let Some(mode_data) = &mut self.mode_data {
            if name == ":global" {
                mode_data.set_global();
                (self.handle_dependency)(Dependency::Replace {
                    content: "",
                    range: Range::new(start, end),
                });
            } else if name == ":local" {
                mode_data.set_local();
                (self.handle_dependency)(Dependency::Replace {
                    content: "",
                    range: Range::new(start, end),
                });
            }
        }
        Some(())
    }

    fn pseudo_class(&mut self, lexer: &mut Lexer<'s>, start: Pos, end: Pos) -> Option<()> {
        let Some(mode_data) = &mut self.mode_data else {
            return Some(());
        };
        let name = lexer.slice(start, end)?.to_ascii_lowercase();
        if name == ":global" || name == ":local" {
            lexer.consume_white_space_and_comments()?;
            let end2 = lexer.cur_pos()?;
            let comments = lexer.slice(end, end2)?.trim_matches(is_white_space);
            (self.handle_dependency)(Dependency::Replace {
                content: comments,
                range: Range::new(start, end2),
            });
            if name == ":global" {
                mode_data.set_global();
            } else {
                mode_data.set_local();
            }
            return Some(());
        }
        if matches!(self.scope, Scope::TopLevel) && name == ":export" {
            self.lex_icss_export(lexer, start)?;
            (self.handle_dependency)(Dependency::Replace {
                content: "",
                range: Range::new(start, lexer.cur_pos()?),
            });
        }
        Some(())
    }

    fn comma(&mut self, _: &mut Lexer, _: Pos, _: Pos) -> Option<()> {
        if let Some(mode_data) = &mut self.mode_data {
            mode_data.set_none();
        }
        Some(())
    }
}