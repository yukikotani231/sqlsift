//! sqlparser's syntax tree converted to the one the analyzer reads ([`sqlsift_ast`]).
//!
//! What the analyzer doesn't model becomes an `Other` node keeping sqlparser's text and
//! location of it and the expressions, queries and relations directly in it.

use std::ops::ControlFlow;

use sqlparser::ast as sp;
use sqlparser::ast::Spanned as _;
use sqlsift_ast as a;

/// A statement, converted.
pub(crate) fn statement(statement: &sp::Statement) -> a::Statement {
    match statement {
        sp::Statement::Query(query) => a::Statement::Query(Box::new(self::query(query))),
        sp::Statement::Insert(insert) => a::Statement::Insert(self::insert(insert)),
        sp::Statement::Update(update) => a::Statement::Update(self::update(update)),
        sp::Statement::Delete(delete) => a::Statement::Delete(self::delete(delete)),
        sp::Statement::Merge(merge) => a::Statement::Merge(a::Merge {
            table: table_factor(&merge.table),
            source: table_factor(&merge.source),
            on: Box::new(expr(&merge.on)),
            clauses: children(&merge.clauses).exprs,
        }),
        sp::Statement::CreateTable(create) => a::Statement::CreateTable(create_table(create)),
        sp::Statement::CreateView(view) => a::Statement::CreateView(a::CreateView {
            or_replace: view.or_replace,
            materialized: view.materialized,
            name: object_name(&view.name),
            columns: view
                .columns
                .iter()
                .map(|column| a::ViewColumnDef {
                    name: ident(&column.name),
                    data_type: column.data_type.as_ref().map(data_type),
                })
                .collect(),
            query: Box::new(query(&view.query)),
            if_not_exists: view.if_not_exists,
            temporary: view.temporary,
        }),
        sp::Statement::CreateIndex(index) => a::Statement::CreateIndex(a::CreateIndex {
            name: index.name.as_ref().map(object_name),
            table_name: object_name(&index.table_name),
            columns: index.columns.iter().map(index_column).collect(),
            unique: index.unique,
            if_not_exists: index.if_not_exists,
            include: idents(&index.include),
            nulls_distinct: index.nulls_distinct,
            predicate: index.predicate.as_ref().map(expr),
        }),
        sp::Statement::CreateType {
            name,
            representation,
        } => a::Statement::CreateType {
            name: object_name(name),
            representation: representation.as_ref().map(|r| match r {
                sp::UserDefinedTypeRepresentation::Composite { attributes } => {
                    a::UserDefinedTypeRepresentation::Composite {
                        attributes: attributes
                            .iter()
                            .map(|attribute| a::UserDefinedTypeCompositeAttributeDef {
                                name: ident(&attribute.name),
                                data_type: data_type(&attribute.data_type),
                                collation: attribute.collation.as_ref().map(object_name),
                            })
                            .collect(),
                    }
                }
                sp::UserDefinedTypeRepresentation::Enum { labels } => {
                    a::UserDefinedTypeRepresentation::Enum {
                        labels: idents(labels),
                    }
                }
                sp::UserDefinedTypeRepresentation::Range { .. } => {
                    a::UserDefinedTypeRepresentation::Range
                }
                sp::UserDefinedTypeRepresentation::SqlDefinition { .. } => {
                    a::UserDefinedTypeRepresentation::SqlDefinition
                }
            }),
        },
        sp::Statement::CreateTrigger(trigger) => a::Statement::CreateTrigger(a::CreateTrigger {
            name: object_name(&trigger.name),
            period: trigger.period.map(|period| match period {
                sp::TriggerPeriod::For => a::TriggerPeriod::For,
                sp::TriggerPeriod::After => a::TriggerPeriod::After,
                sp::TriggerPeriod::Before => a::TriggerPeriod::Before,
                sp::TriggerPeriod::InsteadOf => a::TriggerPeriod::InsteadOf,
            }),
            events: trigger
                .events
                .iter()
                .map(|event| match event {
                    sp::TriggerEvent::Insert => a::TriggerEvent::Insert,
                    sp::TriggerEvent::Update(columns) => a::TriggerEvent::Update(idents(columns)),
                    sp::TriggerEvent::Delete => a::TriggerEvent::Delete,
                    sp::TriggerEvent::Truncate => a::TriggerEvent::Truncate,
                })
                .collect(),
            table_name: object_name(&trigger.table_name),
            condition: trigger.condition.as_ref().map(expr),
        }),
        sp::Statement::AlterTable(alter) => a::Statement::AlterTable(a::AlterTable {
            name: object_name(&alter.name),
            if_exists: alter.if_exists,
            only: alter.only,
            operations: alter.operations.iter().map(alter_table_operation).collect(),
        }),
        sp::Statement::AlterType(alter) => a::Statement::AlterType(a::AlterType {
            name: object_name(&alter.name),
            operation: match &alter.operation {
                sp::AlterTypeOperation::Rename(rename) => {
                    a::AlterTypeOperation::Rename(a::AlterTypeRename {
                        new_name: ident(&rename.new_name),
                    })
                }
                sp::AlterTypeOperation::AddValue(add) => {
                    a::AlterTypeOperation::AddValue(a::AlterTypeAddValue {
                        if_not_exists: add.if_not_exists,
                        value: ident(&add.value),
                        position: add.position.as_ref().map(|position| match position {
                            sp::AlterTypeAddValuePosition::Before(v) => {
                                a::AlterTypeAddValuePosition::Before(ident(v))
                            }
                            sp::AlterTypeAddValuePosition::After(v) => {
                                a::AlterTypeAddValuePosition::After(ident(v))
                            }
                        }),
                    })
                }
                sp::AlterTypeOperation::RenameValue(rename) => {
                    a::AlterTypeOperation::RenameValue(a::AlterTypeRenameValue {
                        from: ident(&rename.from),
                        to: ident(&rename.to),
                    })
                }
            },
        }),
        sp::Statement::Drop {
            object_type,
            if_exists,
            names,
            cascade,
            temporary,
            ..
        } => a::Statement::Drop {
            object_type: self::object_type(*object_type),
            if_exists: *if_exists,
            names: names.iter().map(object_name).collect(),
            cascade: *cascade,
            temporary: *temporary,
        },
        sp::Statement::Set(set) => a::Statement::Set(match set {
            sp::Set::SingleAssignment {
                variable, values, ..
            } => a::Set::SingleAssignment {
                variable: object_name(variable),
                values: values.iter().map(expr).collect(),
            },
            _ => a::Set::Other,
        }),
        sp::Statement::Use(use_statement) => a::Statement::Use(match use_statement {
            sp::Use::Catalog(name) => a::Use::Catalog(object_name(name)),
            sp::Use::Schema(name) => a::Use::Schema(object_name(name)),
            sp::Use::Database(name) => a::Use::Database(object_name(name)),
            sp::Use::Warehouse(name) => a::Use::Warehouse(object_name(name)),
            sp::Use::Role(name) => a::Use::Role(object_name(name)),
            sp::Use::Object(name) => a::Use::Object(object_name(name)),
            sp::Use::Default => a::Use::Default,
            sp::Use::SecondaryRoles(_) => a::Use::SecondaryRoles,
        }),
        sp::Statement::Declare { stmts } => a::Statement::Declare {
            stmts: stmts
                .iter()
                .map(|declare| a::Declare {
                    names: idents(&declare.names),
                    data_type: declare.data_type.as_ref().map(data_type),
                    assignment: declare
                        .assignment
                        .as_ref()
                        .map(|assignment| match assignment {
                            sp::DeclareAssignment::Expr(e)
                            | sp::DeclareAssignment::Default(e)
                            | sp::DeclareAssignment::DuckAssignment(e)
                            | sp::DeclareAssignment::For(e)
                            | sp::DeclareAssignment::MsSqlAssignment(e) => expr(e),
                        }),
                    for_query: declare.for_query.as_ref().map(|q| Box::new(query(q))),
                })
                .collect(),
        },
        other => {
            let children = children(other);
            a::Statement::Other(a::OtherStatement {
                text: other.to_string(),
                span: span(other.span()),
                exprs: children.exprs,
                queries: children.queries,
                relations: children.relations,
            })
        }
    }
}

/// The expressions, queries and relations directly in a node (not in its expressions
/// and queries).
struct Children {
    exprs: Vec<a::Expr>,
    queries: Vec<a::Query>,
    relations: Vec<a::ObjectName>,
}

fn children<N: sp::Visit>(node: &N) -> Children {
    struct Collector {
        depth: usize,
        children: Children,
    }
    impl sp::Visitor for Collector {
        type Break = ();
        fn pre_visit_expr(&mut self, e: &sp::Expr) -> ControlFlow<()> {
            if self.depth == 0 {
                self.children.exprs.push(expr(e));
            }
            self.depth += 1;
            ControlFlow::Continue(())
        }
        fn post_visit_expr(&mut self, _: &sp::Expr) -> ControlFlow<()> {
            self.depth -= 1;
            ControlFlow::Continue(())
        }
        fn pre_visit_query(&mut self, q: &sp::Query) -> ControlFlow<()> {
            if self.depth == 0 {
                self.children.queries.push(query(q));
            }
            self.depth += 1;
            ControlFlow::Continue(())
        }
        fn post_visit_query(&mut self, _: &sp::Query) -> ControlFlow<()> {
            self.depth -= 1;
            ControlFlow::Continue(())
        }
        fn pre_visit_relation(&mut self, relation: &sp::ObjectName) -> ControlFlow<()> {
            if self.depth == 0 {
                self.children.relations.push(object_name(relation));
            }
            ControlFlow::Continue(())
        }
    }
    let mut collector = Collector {
        depth: 0,
        children: Children {
            exprs: Vec::new(),
            queries: Vec::new(),
            relations: Vec::new(),
        },
    };
    let _ = node.visit(&mut collector);
    collector.children
}

pub(crate) fn span(span: sqlparser::tokenizer::Span) -> a::Span {
    a::Span::new(
        a::Location::new(span.start.line, span.start.column),
        a::Location::new(span.end.line, span.end.column),
    )
}

fn token(token: &sp::helpers::attached_token::AttachedToken) -> a::AttachedToken {
    a::AttachedToken::new(span(token.0.span))
}

pub(crate) fn ident(ident: &sp::Ident) -> a::Ident {
    a::Ident {
        value: ident.value.clone(),
        quote_style: ident.quote_style,
        span: span(ident.span),
    }
}

fn idents(idents: &[sp::Ident]) -> Vec<a::Ident> {
    idents.iter().map(ident).collect()
}

pub(crate) fn object_name(name: &sp::ObjectName) -> a::ObjectName {
    a::ObjectName(
        name.0
            .iter()
            .map(|part| match part {
                sp::ObjectNamePart::Identifier(i) => a::ObjectNamePart::Identifier(ident(i)),
                sp::ObjectNamePart::Function(function) => {
                    a::ObjectNamePart::Function(a::ObjectNamePartFunction {
                        name: ident(&function.name),
                        args: function.args.iter().map(function_arg).collect(),
                    })
                }
            })
            .collect(),
    )
}

fn object_type(object_type: sp::ObjectType) -> a::ObjectType {
    match object_type {
        sp::ObjectType::Collation => a::ObjectType::Collation,
        sp::ObjectType::Table => a::ObjectType::Table,
        sp::ObjectType::View => a::ObjectType::View,
        sp::ObjectType::MaterializedView => a::ObjectType::MaterializedView,
        sp::ObjectType::Index => a::ObjectType::Index,
        sp::ObjectType::Schema => a::ObjectType::Schema,
        sp::ObjectType::Database => a::ObjectType::Database,
        sp::ObjectType::Role => a::ObjectType::Role,
        sp::ObjectType::Sequence => a::ObjectType::Sequence,
        sp::ObjectType::Stage => a::ObjectType::Stage,
        sp::ObjectType::Type => a::ObjectType::Type,
        sp::ObjectType::User => a::ObjectType::User,
        sp::ObjectType::Stream => a::ObjectType::Stream,
        sp::ObjectType::Warehouse => a::ObjectType::Warehouse,
        #[allow(unreachable_patterns)]
        _ => a::ObjectType::Other,
    }
}

pub(crate) fn value(value: &sp::Value) -> a::Value {
    match value {
        sp::Value::Number(n, long) => a::Value::Number(n.clone(), *long),
        sp::Value::SingleQuotedString(s) => a::Value::SingleQuotedString(s.clone()),
        sp::Value::DollarQuotedString(s) => a::Value::DollarQuotedString(a::DollarQuotedString {
            value: s.value.clone(),
            tag: s.tag.clone(),
        }),
        sp::Value::TripleSingleQuotedString(s) => a::Value::TripleSingleQuotedString(s.clone()),
        sp::Value::TripleDoubleQuotedString(s) => a::Value::TripleDoubleQuotedString(s.clone()),
        sp::Value::EscapedStringLiteral(s) => a::Value::EscapedStringLiteral(s.clone()),
        sp::Value::UnicodeStringLiteral(s) => a::Value::UnicodeStringLiteral(s.clone()),
        sp::Value::SingleQuotedByteStringLiteral(s) => {
            a::Value::SingleQuotedByteStringLiteral(s.clone())
        }
        sp::Value::DoubleQuotedByteStringLiteral(s) => {
            a::Value::DoubleQuotedByteStringLiteral(s.clone())
        }
        sp::Value::TripleSingleQuotedByteStringLiteral(s) => {
            a::Value::TripleSingleQuotedByteStringLiteral(s.clone())
        }
        sp::Value::TripleDoubleQuotedByteStringLiteral(s) => {
            a::Value::TripleDoubleQuotedByteStringLiteral(s.clone())
        }
        sp::Value::SingleQuotedRawStringLiteral(s) => {
            a::Value::SingleQuotedRawStringLiteral(s.clone())
        }
        sp::Value::DoubleQuotedRawStringLiteral(s) => {
            a::Value::DoubleQuotedRawStringLiteral(s.clone())
        }
        sp::Value::TripleSingleQuotedRawStringLiteral(s) => {
            a::Value::TripleSingleQuotedRawStringLiteral(s.clone())
        }
        sp::Value::TripleDoubleQuotedRawStringLiteral(s) => {
            a::Value::TripleDoubleQuotedRawStringLiteral(s.clone())
        }
        sp::Value::NationalStringLiteral(s) => a::Value::NationalStringLiteral(s.clone()),
        sp::Value::QuoteDelimitedStringLiteral(s) => {
            a::Value::QuoteDelimitedStringLiteral(s.to_string())
        }
        sp::Value::NationalQuoteDelimitedStringLiteral(s) => {
            a::Value::NationalQuoteDelimitedStringLiteral(s.to_string())
        }
        sp::Value::HexStringLiteral(s) => a::Value::HexStringLiteral(s.clone()),
        sp::Value::DoubleQuotedString(s) => a::Value::DoubleQuotedString(s.clone()),
        sp::Value::Boolean(b) => a::Value::Boolean(*b),
        sp::Value::Null => a::Value::Null,
        sp::Value::Placeholder(s) => a::Value::Placeholder(s.clone()),
    }
}

fn value_with_span(v: &sp::ValueWithSpan) -> a::ValueWithSpan {
    value(&v.value).with_span(span(v.span))
}

fn character_length(length: &sp::CharacterLength) -> a::CharacterLength {
    match length {
        sp::CharacterLength::IntegerLength { length, unit } => a::CharacterLength::IntegerLength {
            length: *length,
            unit: unit.as_ref().map(ToString::to_string),
        },
        sp::CharacterLength::Max => a::CharacterLength::Max,
    }
}

fn number_info(info: &sp::ExactNumberInfo) -> a::ExactNumberInfo {
    match info {
        sp::ExactNumberInfo::None => a::ExactNumberInfo::None,
        sp::ExactNumberInfo::Precision(p) => a::ExactNumberInfo::Precision(*p),
        sp::ExactNumberInfo::PrecisionAndScale(p, s) => {
            a::ExactNumberInfo::PrecisionAndScale(*p, *s)
        }
    }
}

fn time_zone(tz: sp::TimezoneInfo) -> a::TimezoneInfo {
    match tz {
        sp::TimezoneInfo::None => a::TimezoneInfo::None,
        sp::TimezoneInfo::WithTimeZone => a::TimezoneInfo::WithTimeZone,
        sp::TimezoneInfo::WithoutTimeZone => a::TimezoneInfo::WithoutTimeZone,
        sp::TimezoneInfo::Tz => a::TimezoneInfo::Tz,
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn data_type(data_type: &sp::DataType) -> a::DataType {
    use a::DataType as A;
    use sp::DataType as S;
    let chars = |l: &Option<sp::CharacterLength>| l.as_ref().map(character_length);
    match data_type {
        S::Character(l) => A::Character(chars(l)),
        S::Char(l) => A::Char(chars(l)),
        S::CharacterVarying(l) | S::CharVarying(l) => A::CharacterVarying(chars(l)),
        S::Varchar(l) => A::Varchar(chars(l)),
        S::Nvarchar(l) => A::Nvarchar(chars(l)),
        S::Uuid => A::Uuid,
        S::Clob(n) => A::Clob(*n),
        S::Binary(n) => A::Binary(*n),
        S::Varbinary(n) => A::Varbinary(n.as_ref().and_then(|n| match n {
            sp::BinaryLength::IntegerLength { length } => Some(*length),
            sp::BinaryLength::Max => None,
        })),
        S::Blob(n) => A::Blob(*n),
        S::TinyBlob => A::TinyBlob,
        S::MediumBlob => A::MediumBlob,
        S::LongBlob => A::LongBlob,
        S::Bytes(n) => A::Bytes(*n),
        S::Numeric(i) => A::Numeric(number_info(i)),
        S::Decimal(i) => A::Decimal(number_info(i)),
        S::DecimalUnsigned(i) => A::DecimalUnsigned(number_info(i)),
        S::BigNumeric(i) => A::BigNumeric(number_info(i)),
        S::BigDecimal(i) => A::BigDecimal(number_info(i)),
        S::Dec(i) => A::Dec(number_info(i)),
        S::DecUnsigned(i) => A::DecUnsigned(number_info(i)),
        S::Float(i) => A::Float(number_info(i)),
        S::FloatUnsigned(i) => A::FloatUnsigned(number_info(i)),
        S::TinyInt(n) => A::TinyInt(*n),
        S::TinyIntUnsigned(n) => A::TinyIntUnsigned(*n),
        S::Int2(n) => A::Int2(*n),
        S::Int2Unsigned(n) => A::Int2Unsigned(*n),
        S::SmallInt(n) => A::SmallInt(*n),
        S::SmallIntUnsigned(n) => A::SmallIntUnsigned(*n),
        S::MediumInt(n) => A::MediumInt(*n),
        S::MediumIntUnsigned(n) => A::MediumIntUnsigned(*n),
        S::Int(n) => A::Int(*n),
        S::Int4(n) => A::Int4(*n),
        S::Int8(n) => A::Int8(*n),
        S::Int64 => A::Int64,
        S::Integer(n) => A::Integer(*n),
        S::IntUnsigned(n) => A::IntUnsigned(*n),
        S::Int4Unsigned(n) => A::Int4Unsigned(*n),
        S::IntegerUnsigned(n) => A::IntegerUnsigned(*n),
        S::BigInt(n) => A::BigInt(*n),
        S::BigIntUnsigned(n) => A::BigIntUnsigned(*n),
        S::Int8Unsigned(n) => A::Int8Unsigned(*n),
        S::HugeInt => A::HugeInt,
        S::UHugeInt => A::UHugeInt,
        S::UTinyInt => A::UTinyInt,
        S::USmallInt => A::USmallInt,
        S::UBigInt => A::UBigInt,
        S::Signed => A::Signed,
        S::SignedInteger => A::SignedInteger,
        S::Unsigned => A::Unsigned,
        S::UnsignedInteger => A::UnsignedInteger,
        S::Float4 => A::Float4,
        S::Float32 => A::Float32,
        S::Float64 => A::Float64,
        S::Real => A::Real,
        S::RealUnsigned => A::RealUnsigned,
        S::Float8 => A::Float8,
        S::Double(i) => A::Double(number_info(i)),
        S::DoubleUnsigned(i) => A::DoubleUnsigned(number_info(i)),
        S::DoublePrecision => A::DoublePrecision,
        S::DoublePrecisionUnsigned => A::DoublePrecisionUnsigned,
        S::Bool => A::Bool,
        S::Boolean => A::Boolean,
        S::Date => A::Date,
        S::Time(p, tz) => A::Time(*p, time_zone(*tz)),
        S::Datetime(p) => A::Datetime(*p),
        S::Timestamp(p, tz) => A::Timestamp(*p, time_zone(*tz)),
        S::TimestampNtz(p) => A::TimestampNtz(*p),
        S::Interval { .. } => A::Interval(data_type.to_string()),
        S::JSON => A::JSON,
        S::JSONB => A::JSONB,
        S::Regclass => A::Regclass,
        S::Text => A::Text,
        S::TinyText => A::TinyText,
        S::MediumText => A::MediumText,
        S::LongText => A::LongText,
        S::String(n) => A::String(*n),
        S::Bytea => A::Bytea,
        S::VarBit(n) => A::VarBit(*n),
        S::AnyType => A::AnyType,
        S::TsVector => A::TsVector,
        S::TsQuery => A::TsQuery,
        S::GeometricType(_) => A::GeometricType(data_type.to_string()),
        S::Map(..) => A::Map(data_type.to_string()),
        S::Struct(..) => A::Struct(data_type.to_string()),
        S::Custom(name, modifiers) => A::Custom(object_name(name), modifiers.clone()),
        S::Array(element) => A::Array(match element {
            sp::ArrayElemTypeDef::None => a::ArrayElemTypeDef::None,
            sp::ArrayElemTypeDef::AngleBracket(t) => {
                a::ArrayElemTypeDef::AngleBracket(Box::new(self::data_type(t)))
            }
            sp::ArrayElemTypeDef::SquareBracket(t, n) => {
                a::ArrayElemTypeDef::SquareBracket(Box::new(self::data_type(t)), *n)
            }
            sp::ArrayElemTypeDef::Parenthesis(t) => {
                a::ArrayElemTypeDef::Parenthesis(Box::new(self::data_type(t)))
            }
            sp::ArrayElemTypeDef::Qualified(t, n) => {
                a::ArrayElemTypeDef::Qualified(Box::new(self::data_type(t)), *n)
            }
        }),
        S::Enum(members, bits) => A::Enum(
            members
                .iter()
                .map(|member| match member {
                    sp::EnumMember::Name(name) => a::EnumMember::Name(name.clone()),
                    sp::EnumMember::NamedValue(name, value) => {
                        a::EnumMember::NamedValue(name.clone(), expr(value))
                    }
                })
                .collect(),
            *bits,
        ),
        other => A::Other(other.to_string()),
    }
}

fn boxed(e: &sp::Expr) -> Box<a::Expr> {
    Box::new(expr(e))
}

fn opt_boxed(e: Option<&sp::Expr>) -> Option<Box<a::Expr>> {
    e.map(boxed)
}

fn binary_operator(op: &sp::BinaryOperator) -> a::BinaryOperator {
    use a::BinaryOperator as A;
    use sp::BinaryOperator as S;
    match op {
        S::Plus => A::Plus,
        S::Minus => A::Minus,
        S::Multiply => A::Multiply,
        S::Divide => A::Divide,
        S::Modulo => A::Modulo,
        S::StringConcat => A::StringConcat,
        S::Gt => A::Gt,
        S::Lt => A::Lt,
        S::GtEq => A::GtEq,
        S::LtEq => A::LtEq,
        S::Spaceship => A::Spaceship,
        S::Eq => A::Eq,
        S::NotEq => A::NotEq,
        S::And => A::And,
        S::Or => A::Or,
        S::Xor => A::Xor,
        S::BitwiseOr => A::BitwiseOr,
        S::BitwiseAnd => A::BitwiseAnd,
        S::BitwiseXor => A::BitwiseXor,
        S::DuckIntegerDivide => A::DuckIntegerDivide,
        S::MyIntegerDivide => A::MyIntegerDivide,
        S::Match => A::Match,
        S::Regexp => A::Regexp,
        S::Glob => A::Glob,
        S::Custom(s) => A::Custom(s.clone()),
        S::PGBitwiseXor => A::PGBitwiseXor,
        S::PGBitwiseShiftLeft => A::PGBitwiseShiftLeft,
        S::PGBitwiseShiftRight => A::PGBitwiseShiftRight,
        S::PGExp => A::PGExp,
        S::PGOverlap => A::PGOverlap,
        S::PGRegexMatch => A::PGRegexMatch,
        S::PGRegexIMatch => A::PGRegexIMatch,
        S::PGRegexNotMatch => A::PGRegexNotMatch,
        S::PGRegexNotIMatch => A::PGRegexNotIMatch,
        S::PGLikeMatch => A::PGLikeMatch,
        S::PGILikeMatch => A::PGILikeMatch,
        S::PGNotLikeMatch => A::PGNotLikeMatch,
        S::PGNotILikeMatch => A::PGNotILikeMatch,
        S::PGStartsWith => A::PGStartsWith,
        S::Arrow => A::Arrow,
        S::LongArrow => A::LongArrow,
        S::HashArrow => A::HashArrow,
        S::HashLongArrow => A::HashLongArrow,
        S::AtAt => A::AtAt,
        S::AtArrow => A::AtArrow,
        S::ArrowAt => A::ArrowAt,
        S::HashMinus => A::HashMinus,
        S::AtQuestion => A::AtQuestion,
        S::Question => A::Question,
        S::QuestionAnd => A::QuestionAnd,
        S::QuestionPipe => A::QuestionPipe,
        S::PGCustomBinaryOperator(parts) => A::PGCustomBinaryOperator(parts.clone()),
        S::Overlaps => A::Overlaps,
        S::DoubleHash => A::DoubleHash,
        S::LtDashGt => A::LtDashGt,
        S::AndLt => A::AndLt,
        S::AndGt => A::AndGt,
        S::LtLtPipe => A::LtLtPipe,
        S::PipeGtGt => A::PipeGtGt,
        S::AndLtPipe => A::AndLtPipe,
        S::PipeAndGt => A::PipeAndGt,
        S::LtCaret => A::LtCaret,
        S::GtCaret => A::GtCaret,
        S::QuestionHash => A::QuestionHash,
        S::QuestionDash => A::QuestionDash,
        S::QuestionDashPipe => A::QuestionDashPipe,
        S::QuestionDoublePipe => A::QuestionDoublePipe,
        S::At => A::At,
        S::TildeEq => A::TildeEq,
        S::Assignment => A::Assignment,
    }
}

fn unary_operator(op: sp::UnaryOperator) -> a::UnaryOperator {
    use a::UnaryOperator as A;
    use sp::UnaryOperator as S;
    match op {
        S::AtDashAt => A::AtDashAt,
        S::BangNot => A::BangNot,
        S::BitwiseNot => A::BitwiseNot,
        S::DoubleAt => A::DoubleAt,
        S::Hash => A::Hash,
        S::Plus => A::Plus,
        S::Minus => A::Minus,
        S::Not => A::Not,
        S::PGAbs => A::PGAbs,
        S::PGCubeRoot => A::PGCubeRoot,
        S::PGPostfixFactorial => A::PGPostfixFactorial,
        S::PGPrefixFactorial => A::PGPrefixFactorial,
        S::PGSquareRoot => A::PGSquareRoot,
        S::QuestionDash => A::QuestionDash,
        S::QuestionPipe => A::QuestionPipe,
    }
}

fn ceil_floor(field: &sp::CeilFloorKind) -> a::CeilFloorKind {
    match field {
        sp::CeilFloorKind::DateTimeField(sp::DateTimeField::NoDateTime) => {
            a::CeilFloorKind::DateTimeField(String::new())
        }
        sp::CeilFloorKind::DateTimeField(field) => {
            a::CeilFloorKind::DateTimeField(field.to_string())
        }
        sp::CeilFloorKind::Scale(scale) => a::CeilFloorKind::Scale(value_with_span(scale)),
    }
}

/// An expression, converted.
#[allow(clippy::too_many_lines)]
pub(crate) fn expr(e: &sp::Expr) -> a::Expr {
    // A chain of left-associative binary operators (`a OR b OR c ...`) nests as deep
    // as it is long: its left spine is converted bottom-up, without recursing
    let mut spine = Vec::new();
    let mut current = e;
    while let sp::Expr::BinaryOp { left, op, right } = current {
        spine.push((op, right));
        current = left;
    }
    let mut converted = expr_node(current);
    for (op, right) in spine.into_iter().rev() {
        converted = a::Expr::BinaryOp {
            left: Box::new(converted),
            op: binary_operator(op),
            right: boxed(right),
        };
    }
    converted
}

fn expr_node(e: &sp::Expr) -> a::Expr {
    use a::Expr as A;
    use sp::Expr as S;
    match e {
        S::Identifier(i) => A::Identifier(ident(i)),
        S::CompoundIdentifier(parts) => A::CompoundIdentifier(idents(parts)),
        S::CompoundFieldAccess { root, access_chain } => A::CompoundFieldAccess {
            root: boxed(root),
            access_chain: access_chain
                .iter()
                .map(|access| match access {
                    sp::AccessExpr::Dot(e) => a::AccessExpr::Dot(expr(e)),
                    sp::AccessExpr::Subscript(subscript) => {
                        a::AccessExpr::Subscript(self::subscript(subscript))
                    }
                })
                .collect(),
        },
        S::JsonAccess { value, path } => A::JsonAccess {
            value: boxed(value),
            path: a::JsonPath {
                path: path
                    .path
                    .iter()
                    .map(|elem| match elem {
                        sp::JsonPathElem::Dot { key, quoted } => a::JsonPathElem::Dot {
                            key: key.clone(),
                            quoted: *quoted,
                        },
                        sp::JsonPathElem::Bracket { key } => {
                            a::JsonPathElem::Bracket { key: expr(key) }
                        }
                        sp::JsonPathElem::ColonBracket { key } => {
                            a::JsonPathElem::ColonBracket { key: expr(key) }
                        }
                    })
                    .collect(),
            },
        },
        S::IsFalse(e) => A::IsFalse(boxed(e)),
        S::IsNotFalse(e) => A::IsNotFalse(boxed(e)),
        S::IsTrue(e) => A::IsTrue(boxed(e)),
        S::IsNotTrue(e) => A::IsNotTrue(boxed(e)),
        S::IsNull(e) => A::IsNull(boxed(e)),
        S::IsNotNull(e) => A::IsNotNull(boxed(e)),
        S::IsUnknown(e) => A::IsUnknown(boxed(e)),
        S::IsNotUnknown(e) => A::IsNotUnknown(boxed(e)),
        S::IsDistinctFrom(l, r) => A::IsDistinctFrom(boxed(l), boxed(r)),
        S::IsNotDistinctFrom(l, r) => A::IsNotDistinctFrom(boxed(l), boxed(r)),
        S::InList {
            expr: e,
            list,
            negated,
        } => A::InList {
            expr: boxed(e),
            list: list.iter().map(expr).collect(),
            negated: *negated,
        },
        S::InSubquery {
            expr: e,
            subquery,
            negated,
        } => A::InSubquery {
            expr: boxed(e),
            subquery: Box::new(query(subquery)),
            negated: *negated,
        },
        S::Between {
            expr: e,
            negated,
            low,
            high,
        } => A::Between {
            expr: boxed(e),
            negated: *negated,
            low: boxed(low),
            high: boxed(high),
        },
        S::BinaryOp { left, op, right } => A::BinaryOp {
            left: boxed(left),
            op: binary_operator(op),
            right: boxed(right),
        },
        S::Like {
            negated,
            any,
            expr: e,
            pattern,
            escape_char,
        } => A::Like {
            negated: *negated,
            any: *any,
            expr: boxed(e),
            pattern: boxed(pattern),
            escape_char: escape_char.as_ref().map(|c| Box::new(expr(c))),
        },
        S::ILike {
            negated,
            any,
            expr: e,
            pattern,
            escape_char,
        } => A::ILike {
            negated: *negated,
            any: *any,
            expr: boxed(e),
            pattern: boxed(pattern),
            escape_char: escape_char.as_ref().map(|c| Box::new(expr(c))),
        },
        S::SimilarTo {
            negated,
            expr: e,
            pattern,
            escape_char,
        } => A::SimilarTo {
            negated: *negated,
            expr: boxed(e),
            pattern: boxed(pattern),
            escape_char: escape_char.as_ref().map(|c| Box::new(expr(c))),
        },
        S::RLike {
            negated,
            expr: e,
            pattern,
            regexp,
        } => A::RLike {
            negated: *negated,
            expr: boxed(e),
            pattern: boxed(pattern),
            regexp: *regexp,
        },
        S::AnyOp {
            left,
            compare_op,
            right,
            is_some,
        } => A::AnyOp {
            left: boxed(left),
            compare_op: binary_operator(compare_op),
            right: boxed(right),
            is_some: *is_some,
        },
        S::AllOp {
            left,
            compare_op,
            right,
        } => A::AllOp {
            left: boxed(left),
            compare_op: binary_operator(compare_op),
            right: boxed(right),
        },
        S::UnaryOp { op, expr: e } => A::UnaryOp {
            op: unary_operator(*op),
            expr: boxed(e),
        },
        S::Cast {
            kind,
            expr: e,
            data_type: t,
            format,
        } => A::Cast {
            kind: match kind {
                sp::CastKind::Cast => a::CastKind::Cast,
                sp::CastKind::TryCast => a::CastKind::TryCast,
                sp::CastKind::SafeCast => a::CastKind::SafeCast,
                sp::CastKind::DoubleColon => a::CastKind::DoubleColon,
            },
            expr: boxed(e),
            data_type: data_type(t),
            format: format.as_ref().map(ToString::to_string),
        },
        S::AtTimeZone {
            timestamp,
            time_zone,
        } => A::AtTimeZone {
            timestamp: boxed(timestamp),
            time_zone: boxed(time_zone),
        },
        S::Extract {
            field,
            syntax,
            expr: e,
        } => A::Extract {
            field: field.to_string(),
            syntax: match syntax {
                sp::ExtractSyntax::From => a::ExtractSyntax::From,
                sp::ExtractSyntax::Comma => a::ExtractSyntax::Comma,
            },
            expr: boxed(e),
        },
        S::Ceil { expr: e, field } => A::Ceil {
            expr: boxed(e),
            field: ceil_floor(field),
        },
        S::Floor { expr: e, field } => A::Floor {
            expr: boxed(e),
            field: ceil_floor(field),
        },
        S::Position { expr: e, r#in } => A::Position {
            expr: boxed(e),
            r#in: boxed(r#in),
        },
        S::Substring {
            expr: e,
            substring_from,
            substring_for,
            special,
            shorthand,
        } => A::Substring {
            expr: boxed(e),
            substring_from: opt_boxed(substring_from.as_deref()),
            substring_for: opt_boxed(substring_for.as_deref()),
            special: *special,
            shorthand: *shorthand,
        },
        S::Trim {
            expr: e,
            trim_where,
            trim_what,
            trim_characters,
        } => A::Trim {
            trim_where: trim_where.as_ref().map(|w| match w {
                sp::TrimWhereField::Both => a::TrimWhereField::Both,
                sp::TrimWhereField::Leading => a::TrimWhereField::Leading,
                sp::TrimWhereField::Trailing => a::TrimWhereField::Trailing,
            }),
            trim_what: opt_boxed(trim_what.as_deref()),
            expr: boxed(e),
            trim_characters: trim_characters
                .as_ref()
                .map(|chars| chars.iter().map(expr).collect()),
        },
        S::Overlay {
            expr: e,
            overlay_what,
            overlay_from,
            overlay_for,
        } => A::Overlay {
            expr: boxed(e),
            overlay_what: boxed(overlay_what),
            overlay_from: boxed(overlay_from),
            overlay_for: opt_boxed(overlay_for.as_deref()),
        },
        S::Collate { expr: e, collation } => A::Collate {
            expr: boxed(e),
            collation: object_name(collation),
        },
        S::Nested(e) => A::Nested(boxed(e)),
        S::Value(v) => A::Value(value_with_span(v)),
        S::TypedString(typed) => A::TypedString(a::TypedString {
            data_type: data_type(&typed.data_type),
            value: value_with_span(&typed.value),
            uses_odbc_syntax: typed.uses_odbc_syntax,
        }),
        S::Function(f) => A::Function(function(f)),
        S::Case {
            case_token,
            end_token,
            operand,
            conditions,
            else_result,
        } => A::Case {
            case_token: token(case_token),
            end_token: token(end_token),
            operand: opt_boxed(operand.as_deref()),
            conditions: conditions
                .iter()
                .map(|when| a::CaseWhen {
                    condition: expr(&when.condition),
                    result: expr(&when.result),
                })
                .collect(),
            else_result: opt_boxed(else_result.as_deref()),
        },
        S::Exists { subquery, negated } => A::Exists {
            subquery: Box::new(query(subquery)),
            negated: *negated,
        },
        S::Subquery(q) => A::Subquery(Box::new(query(q))),
        S::GroupingSets(sets) => A::GroupingSets(expr_lists(sets)),
        S::Cube(sets) => A::Cube(expr_lists(sets)),
        S::Rollup(sets) => A::Rollup(expr_lists(sets)),
        S::Tuple(exprs) => A::Tuple(exprs.iter().map(expr).collect()),
        S::Array(array) => A::Array(a::Array {
            elem: array.elem.iter().map(expr).collect(),
            named: array.named,
        }),
        S::Interval(interval) => A::Interval(a::Interval {
            value: boxed(&interval.value),
            qualifier: {
                let text = e.to_string();
                let value = format!("INTERVAL {}", interval.value);
                text.strip_prefix(&value)
                    .map(str::trim_start)
                    .filter(|rest| !rest.is_empty())
                    .map(str::to_string)
            },
        }),
        S::Wildcard(t) => A::Wildcard(token(t)),
        S::QualifiedWildcard(name, t) => A::QualifiedWildcard(object_name(name), token(t)),
        other => {
            let children = children_of_expr(other);
            A::Other(a::OtherExpr {
                text: other.to_string(),
                span: span(other.span()),
                exprs: children.exprs,
                queries: children.queries,
            })
        }
    }
}

/// The expressions and queries directly in an expression.
fn children_of_expr(e: &sp::Expr) -> Children {
    // The visitor reports the expression itself first: collect one level below it
    struct Collector {
        depth: usize,
        children: Children,
    }
    impl sp::Visitor for Collector {
        type Break = ();
        fn pre_visit_expr(&mut self, e: &sp::Expr) -> ControlFlow<()> {
            if self.depth == 1 {
                self.children.exprs.push(expr(e));
            }
            self.depth += 1;
            ControlFlow::Continue(())
        }
        fn post_visit_expr(&mut self, _: &sp::Expr) -> ControlFlow<()> {
            self.depth -= 1;
            ControlFlow::Continue(())
        }
        fn pre_visit_query(&mut self, q: &sp::Query) -> ControlFlow<()> {
            if self.depth == 1 {
                self.children.queries.push(query(q));
            }
            self.depth += 1;
            ControlFlow::Continue(())
        }
        fn post_visit_query(&mut self, _: &sp::Query) -> ControlFlow<()> {
            self.depth -= 1;
            ControlFlow::Continue(())
        }
    }
    let mut collector = Collector {
        depth: 0,
        children: Children {
            exprs: Vec::new(),
            queries: Vec::new(),
            relations: Vec::new(),
        },
    };
    let _ = sp::Visit::visit(e, &mut collector);
    collector.children
}

/// An escape character (`LIKE p ESCAPE '\'`) as an expression.
fn value_expr(v: &sp::ValueWithSpan) -> a::Expr {
    a::Expr::Value(value_with_span(v))
}

fn expr_lists(sets: &[Vec<sp::Expr>]) -> Vec<Vec<a::Expr>> {
    sets.iter()
        .map(|set| set.iter().map(expr).collect())
        .collect()
}

fn subscript(subscript: &sp::Subscript) -> a::Subscript {
    match subscript {
        sp::Subscript::Index { index } => a::Subscript::Index { index: expr(index) },
        sp::Subscript::Slice {
            lower_bound,
            upper_bound,
            stride,
        } => a::Subscript::Slice {
            lower_bound: lower_bound.as_ref().map(expr),
            upper_bound: upper_bound.as_ref().map(expr),
            stride: stride.as_ref().map(expr),
        },
    }
}

pub(crate) fn function(f: &sp::Function) -> a::Function {
    a::Function {
        name: object_name(&f.name),
        uses_odbc_syntax: f.uses_odbc_syntax,
        parameters: function_arguments(&f.parameters),
        args: function_arguments(&f.args),
        within_group: f.within_group.iter().map(order_by_expr).collect(),
        filter: opt_boxed(f.filter.as_deref()),
        null_treatment: f.null_treatment.map(null_treatment),
        over: f.over.as_ref().map(|over| match over {
            sp::WindowType::WindowSpec(spec) => a::WindowType::WindowSpec(window_spec(spec)),
            sp::WindowType::NamedWindow(name) => a::WindowType::NamedWindow(ident(name)),
        }),
    }
}

fn null_treatment(treatment: sp::NullTreatment) -> a::NullTreatment {
    match treatment {
        sp::NullTreatment::IgnoreNulls => a::NullTreatment::IgnoreNulls,
        sp::NullTreatment::RespectNulls => a::NullTreatment::RespectNulls,
    }
}

fn window_spec(spec: &sp::WindowSpec) -> a::WindowSpec {
    let bound = |bound: &sp::WindowFrameBound| match bound {
        sp::WindowFrameBound::CurrentRow => a::WindowFrameBound::CurrentRow,
        sp::WindowFrameBound::Preceding(n) => {
            a::WindowFrameBound::Preceding(opt_boxed(n.as_deref()))
        }
        sp::WindowFrameBound::Following(n) => {
            a::WindowFrameBound::Following(opt_boxed(n.as_deref()))
        }
    };
    a::WindowSpec {
        window_name: spec.window_name.as_ref().map(ident),
        partition_by: spec.partition_by.iter().map(expr).collect(),
        order_by: spec.order_by.iter().map(order_by_expr).collect(),
        window_frame: spec.window_frame.as_ref().map(|frame| a::WindowFrame {
            units: match frame.units {
                sp::WindowFrameUnits::Rows => a::WindowFrameUnits::Rows,
                sp::WindowFrameUnits::Range => a::WindowFrameUnits::Range,
                sp::WindowFrameUnits::Groups => a::WindowFrameUnits::Groups,
            },
            start_bound: bound(&frame.start_bound),
            end_bound: frame.end_bound.as_ref().map(bound),
        }),
    }
}

fn function_arguments(args: &sp::FunctionArguments) -> a::FunctionArguments {
    match args {
        sp::FunctionArguments::None => a::FunctionArguments::None,
        sp::FunctionArguments::Subquery(q) => a::FunctionArguments::Subquery(Box::new(query(q))),
        sp::FunctionArguments::List(list) => a::FunctionArguments::List(a::FunctionArgumentList {
            duplicate_treatment: list.duplicate_treatment.map(|d| match d {
                sp::DuplicateTreatment::Distinct => a::DuplicateTreatment::Distinct,
                sp::DuplicateTreatment::All => a::DuplicateTreatment::All,
            }),
            args: list.args.iter().map(function_arg).collect(),
            clauses: list
                .clauses
                .iter()
                .map(|clause| match clause {
                    sp::FunctionArgumentClause::IgnoreOrRespectNulls(t) => {
                        a::FunctionArgumentClause::IgnoreOrRespectNulls(null_treatment(*t))
                    }
                    sp::FunctionArgumentClause::OrderBy(order_by) => {
                        a::FunctionArgumentClause::OrderBy(
                            order_by.iter().map(order_by_expr).collect(),
                        )
                    }
                    sp::FunctionArgumentClause::Limit(limit) => {
                        a::FunctionArgumentClause::Limit(expr(limit))
                    }
                    sp::FunctionArgumentClause::Separator(separator) => {
                        a::FunctionArgumentClause::Separator(value_with_span(separator))
                    }
                    other => {
                        a::FunctionArgumentClause::Other(other.to_string(), children(other).exprs)
                    }
                })
                .collect(),
        }),
    }
}

pub(crate) fn function_arg(arg: &sp::FunctionArg) -> a::FunctionArg {
    let operator = |operator: &sp::FunctionArgOperator| match operator {
        sp::FunctionArgOperator::Equals => a::FunctionArgOperator::Equals,
        sp::FunctionArgOperator::RightArrow => a::FunctionArgOperator::RightArrow,
        sp::FunctionArgOperator::Assignment => a::FunctionArgOperator::Assignment,
        sp::FunctionArgOperator::Colon => a::FunctionArgOperator::Colon,
        sp::FunctionArgOperator::Value => a::FunctionArgOperator::Value,
        sp::FunctionArgOperator::Space => a::FunctionArgOperator::Space,
    };
    match arg {
        sp::FunctionArg::Named {
            name,
            arg,
            operator: op,
        } => a::FunctionArg::Named {
            name: ident(name),
            arg: function_arg_expr(arg),
            operator: operator(op),
        },
        sp::FunctionArg::ExprNamed {
            name,
            arg,
            operator: op,
        } => a::FunctionArg::ExprNamed {
            name: expr(name),
            arg: function_arg_expr(arg),
            operator: operator(op),
        },
        sp::FunctionArg::Unnamed(arg) => a::FunctionArg::Unnamed(function_arg_expr(arg)),
    }
}

fn function_arg_expr(arg: &sp::FunctionArgExpr) -> a::FunctionArgExpr {
    match arg {
        sp::FunctionArgExpr::Expr(e) => a::FunctionArgExpr::Expr(expr(e)),
        sp::FunctionArgExpr::QualifiedWildcard(name) => {
            a::FunctionArgExpr::QualifiedWildcard(object_name(name))
        }
        sp::FunctionArgExpr::Wildcard | sp::FunctionArgExpr::WildcardWithOptions(_) => {
            a::FunctionArgExpr::Wildcard
        }
    }
}

fn order_by_expr(o: &sp::OrderByExpr) -> a::OrderByExpr {
    a::OrderByExpr {
        expr: expr(&o.expr),
        options: order_by_options(&o.options),
    }
}

fn order_by_options(options: &sp::OrderByOptions) -> a::OrderByOptions {
    a::OrderByOptions {
        sort: options.sort.as_ref().map(|sort| match sort {
            sp::OrderBySort::Asc => a::OrderBySort::Asc,
            sp::OrderBySort::Desc => a::OrderBySort::Desc,
            sp::OrderBySort::Using(op) => a::OrderBySort::Using(object_name(op)),
        }),
        nulls_first: options.nulls_first,
    }
}

/// A query, converted.
pub(crate) fn query(q: &sp::Query) -> a::Query {
    a::Query {
        with: q.with.as_ref().map(|with| a::With {
            with_token: token(&with.with_token),
            recursive: with.recursive,
            cte_tables: with
                .cte_tables
                .iter()
                .map(|cte| a::Cte {
                    alias: table_alias(&cte.alias),
                    query: Box::new(query(&cte.query)),
                    from: cte.from.as_ref().map(ident),
                    materialized: cte.materialized.as_ref().map(|m| match m {
                        sp::CteAsMaterialized::Materialized => a::CteAsMaterialized::Materialized,
                        sp::CteAsMaterialized::NotMaterialized => {
                            a::CteAsMaterialized::NotMaterialized
                        }
                    }),
                    closing_paren_token: token(&cte.closing_paren_token),
                })
                .collect(),
        }),
        body: Box::new(set_expr(&q.body)),
        order_by: q.order_by.as_ref().map(|order_by| a::OrderBy {
            kind: match &order_by.kind {
                sp::OrderByKind::All(options) => a::OrderByKind::All(order_by_options(options)),
                sp::OrderByKind::Expressions(exprs) => {
                    a::OrderByKind::Expressions(exprs.iter().map(order_by_expr).collect())
                }
            },
        }),
        limit_clause: q.limit_clause.as_ref().map(|limit| match limit {
            sp::LimitClause::LimitOffset {
                limit,
                offset,
                limit_by,
            } => a::LimitClause::LimitOffset {
                limit: limit.as_ref().map(expr),
                offset: offset.as_ref().map(|offset| a::Offset {
                    value: expr(&offset.value),
                    rows: match offset.rows {
                        sp::OffsetRows::None => a::OffsetRows::None,
                        sp::OffsetRows::Row => a::OffsetRows::Row,
                        sp::OffsetRows::Rows => a::OffsetRows::Rows,
                    },
                }),
                limit_by: limit_by.iter().map(expr).collect(),
            },
            sp::LimitClause::OffsetCommaLimit { offset, limit } => {
                a::LimitClause::OffsetCommaLimit {
                    offset: expr(offset),
                    limit: expr(limit),
                }
            }
        }),
        fetch: q.fetch.as_ref().map(|fetch| a::Fetch {
            with_ties: fetch.with_ties,
            percent: fetch.percent,
            quantity: fetch.quantity.as_ref().map(expr),
        }),
        locks: q
            .locks
            .iter()
            .map(|lock| a::LockClause {
                text: lock.to_string(),
                of: lock.of.as_ref().map(object_name),
            })
            .collect(),
        other: {
            let mut exprs = Vec::new();
            if let Some(settings) = &q.settings {
                exprs.extend(settings.iter().map(|s| expr(&s.value)));
            }
            exprs.extend(children(&q.pipe_operators).exprs);
            exprs
        },
    }
}

fn set_expr(body: &sp::SetExpr) -> a::SetExpr {
    match body {
        sp::SetExpr::Select(select) => a::SetExpr::Select(Box::new(self::select(select))),
        sp::SetExpr::Query(q) => a::SetExpr::Query(Box::new(query(q))),
        sp::SetExpr::SetOperation {
            left,
            op,
            set_quantifier,
            right,
        } => a::SetExpr::SetOperation {
            left: Box::new(set_expr(left)),
            op: match op {
                sp::SetOperator::Union => a::SetOperator::Union,
                sp::SetOperator::Except => a::SetOperator::Except,
                sp::SetOperator::Intersect => a::SetOperator::Intersect,
                sp::SetOperator::Minus => a::SetOperator::Minus,
            },
            set_quantifier: match set_quantifier {
                sp::SetQuantifier::All => a::SetQuantifier::All,
                sp::SetQuantifier::Distinct => a::SetQuantifier::Distinct,
                sp::SetQuantifier::ByName => a::SetQuantifier::ByName,
                sp::SetQuantifier::AllByName => a::SetQuantifier::AllByName,
                sp::SetQuantifier::DistinctByName => a::SetQuantifier::DistinctByName,
                sp::SetQuantifier::None => a::SetQuantifier::None,
            },
            right: Box::new(set_expr(right)),
        },
        sp::SetExpr::Values(values) => a::SetExpr::Values(self::values(values)),
        sp::SetExpr::Insert(s) => a::SetExpr::Insert(statement(s)),
        sp::SetExpr::Update(s) => a::SetExpr::Update(statement(s)),
        sp::SetExpr::Delete(s) => a::SetExpr::Delete(statement(s)),
        sp::SetExpr::Merge(s) => a::SetExpr::Merge(statement(s)),
        sp::SetExpr::Table(table) => a::SetExpr::Table(Box::new(a::Table {
            table_name: table.table_name.clone(),
            schema_name: table.schema_name.clone(),
        })),
    }
}

pub(crate) fn values(values: &sp::Values) -> a::Values {
    a::Values {
        explicit_row: values.explicit_row,
        value_keyword: values.value_keyword,
        rows: values
            .rows
            .iter()
            .map(|row| a::Parens {
                opening_token: token(&row.opening_token),
                content: row.content.iter().map(expr).collect(),
                closing_token: token(&row.closing_token),
            })
            .collect(),
    }
}

fn select(s: &sp::Select) -> a::Select {
    a::Select {
        select_token: token(&s.select_token),
        distinct: s.distinct.as_ref().map(|d| match d {
            sp::Distinct::All => a::Distinct::All,
            sp::Distinct::Distinct => a::Distinct::Distinct,
            sp::Distinct::On(exprs) => a::Distinct::On(exprs.iter().map(expr).collect()),
        }),
        top: s
            .top
            .as_ref()
            .and_then(|top| top.quantity.as_ref())
            .and_then(|quantity| match quantity {
                sp::TopQuantity::Expr(e) => Some(expr(e)),
                sp::TopQuantity::Constant(_) => None,
            }),
        projection: s.projection.iter().map(select_item).collect(),
        exclude: s.exclude.as_ref().map(exclude_select_item),
        into: s.into.as_ref().map(|into| a::SelectInto {
            temporary: into.temporary,
            unlogged: into.unlogged,
            table: into.table,
            targets: into.targets.iter().map(expr).collect(),
        }),
        from: s.from.iter().map(table_with_joins).collect(),
        lateral_views: s
            .lateral_views
            .iter()
            .map(|view| a::LateralView {
                lateral_view: expr(&view.lateral_view),
                lateral_view_name: object_name(&view.lateral_view_name),
                lateral_col_alias: idents(&view.lateral_col_alias),
                outer: view.outer,
            })
            .collect(),
        prewhere: s.prewhere.as_ref().map(expr),
        selection: s.selection.as_ref().map(expr),
        connect_by: children(&s.connect_by).exprs,
        group_by: match &s.group_by {
            sp::GroupByExpr::All(modifiers) => a::GroupByExpr::All(group_by_modifiers(modifiers)),
            sp::GroupByExpr::Expressions(exprs, modifiers) => a::GroupByExpr::Expressions(
                exprs.iter().map(expr).collect(),
                group_by_modifiers(modifiers),
            ),
        },
        cluster_by: s.cluster_by.iter().map(expr).collect(),
        distribute_by: s.distribute_by.iter().map(expr).collect(),
        sort_by: s.sort_by.iter().map(order_by_expr).collect(),
        having: s.having.as_ref().map(expr),
        named_window: s
            .named_window
            .iter()
            .map(|window| {
                a::NamedWindowDefinition(
                    ident(&window.0),
                    match &window.1 {
                        sp::NamedWindowExpr::NamedWindow(name) => {
                            a::NamedWindowExpr::NamedWindow(ident(name))
                        }
                        sp::NamedWindowExpr::WindowSpec(spec) => {
                            a::NamedWindowExpr::WindowSpec(window_spec(spec))
                        }
                    },
                )
            })
            .collect(),
        qualify: s.qualify.as_ref().map(expr),
        window_before_qualify: s.window_before_qualify,
        value_table_mode: s.value_table_mode.as_ref().map(ToString::to_string),
        from_first: matches!(
            s.flavor,
            sp::SelectFlavor::FromFirst | sp::SelectFlavor::FromFirstNoSelect
        ),
    }
}

fn group_by_modifiers(modifiers: &[sp::GroupByWithModifier]) -> Vec<a::GroupByWithModifier> {
    modifiers
        .iter()
        .map(|modifier| match modifier {
            sp::GroupByWithModifier::Rollup => a::GroupByWithModifier::Rollup,
            sp::GroupByWithModifier::Cube => a::GroupByWithModifier::Cube,
            sp::GroupByWithModifier::Totals => a::GroupByWithModifier::Totals,
            sp::GroupByWithModifier::GroupingSets(e) => {
                a::GroupByWithModifier::GroupingSets(expr(e))
            }
        })
        .collect()
}

fn exclude_select_item(exclude: &sp::ExcludeSelectItem) -> a::ExcludeSelectItem {
    match exclude {
        sp::ExcludeSelectItem::Single(name) => a::ExcludeSelectItem::Single(object_name(name)),
        sp::ExcludeSelectItem::Multiple(names) => {
            a::ExcludeSelectItem::Multiple(names.iter().map(object_name).collect())
        }
    }
}

pub(crate) fn select_item(item: &sp::SelectItem) -> a::SelectItem {
    match item {
        sp::SelectItem::UnnamedExpr(e) => a::SelectItem::UnnamedExpr(expr(e)),
        sp::SelectItem::ExprWithAlias { expr: e, alias } => a::SelectItem::ExprWithAlias {
            expr: expr(e),
            alias: ident(alias),
        },
        sp::SelectItem::ExprWithAliases { expr: e, aliases } => a::SelectItem::ExprWithAliases {
            expr: expr(e),
            aliases: idents(aliases),
        },
        sp::SelectItem::QualifiedWildcard(kind, options) => a::SelectItem::QualifiedWildcard(
            match kind {
                sp::SelectItemQualifiedWildcardKind::ObjectName(name) => {
                    a::SelectItemQualifiedWildcardKind::ObjectName(object_name(name))
                }
                sp::SelectItemQualifiedWildcardKind::Expr(e) => {
                    a::SelectItemQualifiedWildcardKind::Expr(expr(e))
                }
            },
            wildcard_options(options),
        ),
        sp::SelectItem::Wildcard(options) => a::SelectItem::Wildcard(wildcard_options(options)),
    }
}

fn wildcard_options(options: &sp::WildcardAdditionalOptions) -> a::WildcardAdditionalOptions {
    a::WildcardAdditionalOptions {
        wildcard_token: token(&options.wildcard_token),
        opt_ilike: options.opt_ilike.as_ref().map(|ilike| a::IlikeSelectItem {
            pattern: ilike.pattern.clone(),
        }),
        opt_exclude: options.opt_exclude.as_ref().map(exclude_select_item),
        opt_except: options
            .opt_except
            .as_ref()
            .map(|except| a::ExceptSelectItem {
                first_element: ident(&except.first_element),
                additional_elements: idents(&except.additional_elements),
            }),
        opt_replace: options
            .opt_replace
            .as_ref()
            .map(|replace| a::ReplaceSelectItem {
                items: replace
                    .items
                    .iter()
                    .map(|item| {
                        Box::new(a::ReplaceSelectElement {
                            expr: expr(&item.expr),
                            column_name: ident(&item.column_name),
                            as_keyword: item.as_keyword,
                        })
                    })
                    .collect(),
            }),
        opt_rename: options.opt_rename.as_ref().map(|rename| {
            let item = |item: &sp::IdentWithAlias| a::IdentWithAlias {
                ident: ident(&item.ident),
                alias: ident(&item.alias),
            };
            match rename {
                sp::RenameSelectItem::Single(single) => a::RenameSelectItem::Single(item(single)),
                sp::RenameSelectItem::Multiple(items) => {
                    a::RenameSelectItem::Multiple(items.iter().map(item).collect())
                }
            }
        }),
        opt_alias: options.opt_alias.as_ref().map(ident),
    }
}

pub(crate) fn table_with_joins(t: &sp::TableWithJoins) -> a::TableWithJoins {
    a::TableWithJoins {
        relation: table_factor(&t.relation),
        joins: t
            .joins
            .iter()
            .map(|join| a::Join {
                relation: table_factor(&join.relation),
                global: join.global,
                join_operator: join_operator(&join.join_operator),
            })
            .collect(),
    }
}

fn join_operator(op: &sp::JoinOperator) -> a::JoinOperator {
    use a::JoinOperator as A;
    use sp::JoinOperator as S;
    let c = join_constraint;
    match op {
        S::Join(x) => A::Join(c(x)),
        S::Inner(x) => A::Inner(c(x)),
        S::Left(x) => A::Left(c(x)),
        S::LeftOuter(x) => A::LeftOuter(c(x)),
        S::Right(x) => A::Right(c(x)),
        S::RightOuter(x) => A::RightOuter(c(x)),
        S::FullOuter(x) => A::FullOuter(c(x)),
        S::CrossJoin(x) => A::CrossJoin(c(x)),
        S::Semi(x) => A::Semi(c(x)),
        S::LeftSemi(x) => A::LeftSemi(c(x)),
        S::RightSemi(x) => A::RightSemi(c(x)),
        S::Anti(x) => A::Anti(c(x)),
        S::LeftAnti(x) => A::LeftAnti(c(x)),
        S::RightAnti(x) => A::RightAnti(c(x)),
        S::CrossApply => A::CrossApply,
        S::OuterApply => A::OuterApply,
        S::AsOf {
            match_condition,
            constraint,
        } => A::AsOf {
            match_condition: expr(match_condition),
            constraint: c(constraint),
        },
        S::StraightJoin(x) => A::StraightJoin(c(x)),
        S::ArrayJoin => A::ArrayJoin,
        S::LeftArrayJoin => A::LeftArrayJoin,
        S::InnerArrayJoin => A::InnerArrayJoin,
    }
}

fn join_constraint(constraint: &sp::JoinConstraint) -> a::JoinConstraint {
    match constraint {
        sp::JoinConstraint::On(e) => a::JoinConstraint::On(expr(e)),
        sp::JoinConstraint::Using(names) => {
            a::JoinConstraint::Using(names.iter().map(object_name).collect())
        }
        sp::JoinConstraint::Natural => a::JoinConstraint::Natural,
        sp::JoinConstraint::None => a::JoinConstraint::None,
    }
}

fn table_alias(alias: &sp::TableAlias) -> a::TableAlias {
    a::TableAlias {
        explicit: alias.explicit,
        name: ident(&alias.name),
        columns: alias
            .columns
            .iter()
            .map(|column| a::TableAliasColumnDef {
                name: ident(&column.name),
                data_type: column.data_type.as_ref().map(data_type),
            })
            .collect(),
        at: alias.at.as_ref().map(ident),
    }
}

fn opt_alias(alias: Option<&sp::TableAlias>) -> Option<a::TableAlias> {
    alias.map(table_alias)
}

fn expr_with_alias(e: &sp::ExprWithAlias) -> a::ExprWithAlias {
    a::ExprWithAlias {
        expr: expr(&e.expr),
        alias: e.alias.as_ref().map(ident),
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn table_factor(factor: &sp::TableFactor) -> a::TableFactor {
    match factor {
        sp::TableFactor::Table {
            name,
            alias,
            args,
            with_hints,
            version,
            with_ordinality,
            partitions,
            json_path,
            sample,
            ..
        } => a::TableFactor::Table {
            name: object_name(name),
            alias: opt_alias(alias.as_ref()),
            args: args.as_ref().map(|args| a::TableFunctionArgs {
                args: args.args.iter().map(function_arg).collect(),
            }),
            with_hints: with_hints.iter().map(expr).collect(),
            version: children(version).exprs,
            with_ordinality: *with_ordinality,
            partitions: idents(partitions),
            json_path: json_path.as_ref().map(|path| {
                match expr(&sp::Expr::JsonAccess {
                    value: Box::new(sp::Expr::Value(sp::Value::Null.with_empty_span())),
                    path: path.clone(),
                }) {
                    a::Expr::JsonAccess { path, .. } => path,
                    _ => a::JsonPath { path: Vec::new() },
                }
            }),
            sample: children(sample).exprs,
        },
        sp::TableFactor::Derived {
            lateral,
            subquery,
            alias,
            ..
        } => a::TableFactor::Derived {
            lateral: *lateral,
            subquery: Box::new(query(subquery)),
            alias: opt_alias(alias.as_ref()),
        },
        sp::TableFactor::TableFunction { expr: e, alias } => a::TableFactor::TableFunction {
            expr: expr(e),
            alias: opt_alias(alias.as_ref()),
        },
        sp::TableFactor::Function {
            lateral,
            name,
            args,
            with_ordinality,
            alias,
        } => a::TableFactor::Function {
            lateral: *lateral,
            name: object_name(name),
            args: args.iter().map(function_arg).collect(),
            with_ordinality: *with_ordinality,
            alias: opt_alias(alias.as_ref()),
        },
        sp::TableFactor::UNNEST {
            alias,
            array_exprs,
            with_offset,
            with_offset_alias,
            with_ordinality,
        } => a::TableFactor::UNNEST {
            alias: opt_alias(alias.as_ref()),
            array_exprs: array_exprs.iter().map(expr).collect(),
            with_offset: *with_offset,
            with_offset_alias: with_offset_alias.as_ref().map(ident),
            with_ordinality: *with_ordinality,
        },
        sp::TableFactor::JsonTable {
            json_expr,
            json_path,
            columns,
            alias,
        } => a::TableFactor::JsonTable {
            json_expr: expr(json_expr),
            json_path: value_with_span(json_path),
            columns: columns.iter().map(json_table_column).collect(),
            alias: opt_alias(alias.as_ref()),
        },
        sp::TableFactor::OpenJsonTable {
            json_expr,
            json_path,
            alias,
            ..
        } => a::TableFactor::OpenJsonTable {
            json_expr: expr(json_expr),
            json_path: json_path.as_ref().map(value_with_span),
            alias: opt_alias(alias.as_ref()),
        },
        sp::TableFactor::NestedJoin {
            table_with_joins: t,
            alias,
        } => a::TableFactor::NestedJoin {
            table_with_joins: Box::new(table_with_joins(t)),
            alias: opt_alias(alias.as_ref()),
        },
        sp::TableFactor::Pivot {
            table,
            aggregate_functions,
            value_column,
            value_source,
            default_on_null,
            alias,
        } => a::TableFactor::Pivot {
            table: Box::new(table_factor(table)),
            aggregate_functions: aggregate_functions.iter().map(expr_with_alias).collect(),
            value_column: value_column.iter().map(expr).collect(),
            value_source: match value_source {
                sp::PivotValueSource::List(items) => {
                    a::PivotValueSource::List(items.iter().map(expr_with_alias).collect())
                }
                sp::PivotValueSource::Any(items) => {
                    a::PivotValueSource::Any(items.iter().map(order_by_expr).collect())
                }
                sp::PivotValueSource::Subquery(q) => {
                    a::PivotValueSource::Subquery(Box::new(query(q)))
                }
            },
            default_on_null: default_on_null.as_ref().map(expr),
            alias: opt_alias(alias.as_ref()),
        },
        sp::TableFactor::Unpivot {
            table,
            value,
            name,
            columns,
            alias,
            ..
        } => a::TableFactor::Unpivot {
            table: Box::new(table_factor(table)),
            value: expr(value),
            name: ident(name),
            columns: columns.iter().map(expr_with_alias).collect(),
            alias: opt_alias(alias.as_ref()),
        },
        other => {
            let alias = match other {
                sp::TableFactor::MatchRecognize { alias, .. }
                | sp::TableFactor::XmlTable { alias, .. }
                | sp::TableFactor::SemanticView { alias, .. } => opt_alias(alias.as_ref()),
                _ => None,
            };
            let tables = match other {
                sp::TableFactor::MatchRecognize { table, .. } => vec![table_factor(table)],
                _ => Vec::new(),
            };
            let exprs = match other {
                sp::TableFactor::MatchRecognize {
                    partition_by,
                    order_by,
                    measures,
                    symbols,
                    ..
                } => partition_by
                    .iter()
                    .map(expr)
                    .chain(order_by.iter().map(|o| expr(&o.expr)))
                    .chain(measures.iter().map(|m| expr(&m.expr)))
                    .chain(symbols.iter().map(|s| expr(&s.definition)))
                    .collect(),
                _ => children(other).exprs,
            };
            a::TableFactor::Other(a::OtherTableFactor {
                text: other.to_string(),
                span: span(other.span()),
                alias,
                tables,
                exprs,
            })
        }
    }
}

fn json_table_column(column: &sp::JsonTableColumn) -> a::JsonTableColumn {
    match column {
        sp::JsonTableColumn::Named(named) => a::JsonTableColumn::Named(a::JsonTableNamedColumn {
            name: ident(&named.name),
            r#type: data_type(&named.r#type),
            path: value_with_span(&named.path),
            exists: named.exists,
        }),
        sp::JsonTableColumn::ForOrdinality(name) => a::JsonTableColumn::ForOrdinality(ident(name)),
        sp::JsonTableColumn::Nested(nested) => {
            a::JsonTableColumn::Nested(a::JsonTableNestedColumn {
                path: value_with_span(&nested.path),
                columns: nested.columns.iter().map(json_table_column).collect(),
            })
        }
    }
}

fn assignment(assignment: &sp::Assignment) -> a::Assignment {
    a::Assignment {
        target: match &assignment.target {
            sp::AssignmentTarget::ColumnName(name) => {
                a::AssignmentTarget::ColumnName(object_name(name))
            }
            sp::AssignmentTarget::Tuple(names) => {
                a::AssignmentTarget::Tuple(names.iter().map(object_name).collect())
            }
        },
        value: expr(&assignment.value),
    }
}

fn assignments(list: &[sp::Assignment]) -> Vec<a::Assignment> {
    list.iter().map(assignment).collect()
}

fn select_items(items: Option<&Vec<sp::SelectItem>>) -> Option<Vec<a::SelectItem>> {
    items.map(|items| items.iter().map(select_item).collect())
}

fn insert(insert: &sp::Insert) -> a::Insert {
    a::Insert {
        insert_token: token(&insert.insert_token),
        ignore: insert.ignore,
        into: insert.into,
        table: match &insert.table {
            sp::TableObject::TableName(name) => a::TableObject::TableName(object_name(name)),
            sp::TableObject::TableFunction(f) => a::TableObject::TableFunction(function(f)),
            sp::TableObject::TableQuery(q) => a::TableObject::TableQuery(Box::new(query(q))),
        },
        table_alias: insert
            .table_alias
            .as_ref()
            .map(|alias| a::TableAliasWithoutColumns {
                explicit: alias.explicit,
                alias: ident(&alias.alias),
            }),
        columns: insert.columns.iter().map(object_name).collect(),
        overwrite: insert.overwrite,
        source: insert.source.as_ref().map(|q| Box::new(query(q))),
        assignments: assignments(&insert.assignments),
        partitioned: insert
            .partitioned
            .as_ref()
            .map(|exprs| exprs.iter().map(expr).collect()),
        after_columns: idents(&insert.after_columns),
        has_table_keyword: insert.has_table_keyword,
        on: insert.on.as_ref().map(|on| match on {
            sp::OnInsert::DuplicateKeyUpdate(list) => {
                a::OnInsert::DuplicateKeyUpdate(assignments(list))
            }
            sp::OnInsert::OnConflict(on_conflict) => a::OnInsert::OnConflict(a::OnConflict {
                conflict_target: on_conflict
                    .conflict_target
                    .as_ref()
                    .map(|target| match target {
                        sp::ConflictTarget::Columns(columns) => {
                            a::ConflictTarget::Columns(idents(columns))
                        }
                        sp::ConflictTarget::OnConstraint(name) => {
                            a::ConflictTarget::OnConstraint(object_name(name))
                        }
                    }),
                action: match &on_conflict.action {
                    sp::OnConflictAction::DoNothing => a::OnConflictAction::DoNothing,
                    sp::OnConflictAction::DoUpdate(update) => {
                        a::OnConflictAction::DoUpdate(a::DoUpdate {
                            assignments: assignments(&update.assignments),
                            selection: update.selection.as_ref().map(expr),
                        })
                    }
                },
            }),
            #[allow(unreachable_patterns)]
            _ => a::OnInsert::DuplicateKeyUpdate(Vec::new()),
        }),
        returning: select_items(insert.returning.as_ref()),
        replace_into: insert.replace_into,
        or: insert.or.is_some(),
        insert_alias: insert.insert_alias.as_ref().map(|alias| a::InsertAliases {
            row_alias: object_name(&alias.row_alias),
            col_aliases: alias.col_aliases.as_ref().map(|names| idents(names)),
        }),
    }
}

fn update(update: &sp::Update) -> a::Update {
    a::Update {
        update_token: token(&update.update_token),
        table: table_with_joins(&update.table),
        assignments: assignments(&update.assignments),
        from: update.from.as_ref().map(|from| match from {
            sp::UpdateTableFromKind::BeforeSet(from) => {
                a::UpdateTableFromKind::BeforeSet(from.iter().map(table_with_joins).collect())
            }
            sp::UpdateTableFromKind::AfterSet(from) => {
                a::UpdateTableFromKind::AfterSet(from.iter().map(table_with_joins).collect())
            }
        }),
        selection: update.selection.as_ref().map(expr),
        returning: select_items(update.returning.as_ref()),
        order_by: update.order_by.iter().map(order_by_expr).collect(),
        limit: update.limit.as_ref().map(expr),
    }
}

fn delete(delete: &sp::Delete) -> a::Delete {
    a::Delete {
        delete_token: token(&delete.delete_token),
        tables: delete.tables.iter().map(object_name).collect(),
        from: match &delete.from {
            sp::FromTable::WithFromKeyword(from) => {
                a::FromTable::WithFromKeyword(from.iter().map(table_with_joins).collect())
            }
            sp::FromTable::WithoutKeyword(from) => {
                a::FromTable::WithoutKeyword(from.iter().map(table_with_joins).collect())
            }
        },
        using: delete
            .using
            .as_ref()
            .map(|using| using.iter().map(table_with_joins).collect()),
        selection: delete.selection.as_ref().map(expr),
        returning: select_items(delete.returning.as_ref()),
        order_by: delete.order_by.iter().map(order_by_expr).collect(),
        limit: delete.limit.as_ref().map(expr),
    }
}

fn index_column(column: &sp::IndexColumn) -> a::IndexColumn {
    a::IndexColumn {
        column: order_by_expr(&column.column),
        operator_class: column.operator_class.as_ref().map(object_name),
    }
}

fn index_columns(columns: &[sp::IndexColumn]) -> Vec<a::IndexColumn> {
    columns.iter().map(index_column).collect()
}

fn referential_action(action: sp::ReferentialAction) -> a::ReferentialAction {
    match action {
        sp::ReferentialAction::Restrict => a::ReferentialAction::Restrict,
        sp::ReferentialAction::Cascade => a::ReferentialAction::Cascade,
        sp::ReferentialAction::SetNull => a::ReferentialAction::SetNull,
        sp::ReferentialAction::NoAction => a::ReferentialAction::NoAction,
        sp::ReferentialAction::SetDefault => a::ReferentialAction::SetDefault,
    }
}

fn foreign_key(fk: &sp::ForeignKeyConstraint) -> a::ForeignKeyConstraint {
    a::ForeignKeyConstraint {
        name: fk.name.as_ref().map(ident),
        index_name: fk.index_name.as_ref().map(ident),
        columns: idents(&fk.columns),
        foreign_table: object_name(&fk.foreign_table),
        referred_columns: idents(&fk.referred_columns),
        on_delete: fk.on_delete.map(referential_action),
        on_update: fk.on_update.map(referential_action),
    }
}

fn check(check: &sp::CheckConstraint) -> a::CheckConstraint {
    a::CheckConstraint {
        name: check.name.as_ref().map(ident),
        expr: boxed(&check.expr),
        no_inherit: check.no_inherit,
        enforced: check.enforced,
    }
}

fn unique(unique: &sp::UniqueConstraint) -> a::UniqueConstraint {
    a::UniqueConstraint {
        name: unique.name.as_ref().map(ident),
        index_name: unique.index_name.as_ref().map(ident),
        columns: index_columns(&unique.columns),
        nulls_distinct: match unique.nulls_distinct {
            sp::NullsDistinctOption::None => a::NullsDistinctOption::None,
            sp::NullsDistinctOption::Distinct => a::NullsDistinctOption::Distinct,
            sp::NullsDistinctOption::NotDistinct => a::NullsDistinctOption::NotDistinct,
        },
    }
}

fn primary_key(key: &sp::PrimaryKeyConstraint) -> a::PrimaryKeyConstraint {
    a::PrimaryKeyConstraint {
        name: key.name.as_ref().map(ident),
        index_name: key.index_name.as_ref().map(ident),
        columns: index_columns(&key.columns),
    }
}

fn table_constraint(constraint: &sp::TableConstraint) -> a::TableConstraint {
    match constraint {
        sp::TableConstraint::Unique(u) => a::TableConstraint::Unique(unique(u)),
        sp::TableConstraint::PrimaryKey(k) => a::TableConstraint::PrimaryKey(primary_key(k)),
        sp::TableConstraint::ForeignKey(fk) => a::TableConstraint::ForeignKey(foreign_key(fk)),
        sp::TableConstraint::Check(c) => a::TableConstraint::Check(check(c)),
        sp::TableConstraint::Index(index) => a::TableConstraint::Index(a::IndexConstraint {
            name: index.name.as_ref().map(ident),
            columns: index_columns(&index.columns),
        }),
        sp::TableConstraint::FulltextOrSpatial(index) => {
            a::TableConstraint::FulltextOrSpatial(a::IndexConstraint {
                name: index.opt_index_name.as_ref().map(ident),
                columns: index_columns(&index.columns),
            })
        }
        sp::TableConstraint::PrimaryKeyUsingIndex(using) => {
            a::TableConstraint::PrimaryKeyUsingIndex(a::ConstraintUsingIndex {
                name: using.name.as_ref().map(ident),
                index_name: ident(&using.index_name),
            })
        }
        sp::TableConstraint::UniqueUsingIndex(using) => {
            a::TableConstraint::UniqueUsingIndex(a::ConstraintUsingIndex {
                name: using.name.as_ref().map(ident),
                index_name: ident(&using.index_name),
            })
        }
        sp::TableConstraint::Exclude(exclude) => {
            a::TableConstraint::Exclude(a::ExcludeConstraint {
                name: exclude.name.as_ref().map(ident),
                elements: exclude
                    .elements
                    .iter()
                    .map(|element| index_column(&element.column))
                    .collect(),
                predicate: exclude.where_clause.as_deref().map(expr),
            })
        }
    }
}

fn generated_as(generated: sp::GeneratedAs) -> a::GeneratedAs {
    match generated {
        sp::GeneratedAs::Always => a::GeneratedAs::Always,
        sp::GeneratedAs::ByDefault => a::GeneratedAs::ByDefault,
        sp::GeneratedAs::ExpStored => a::GeneratedAs::ExpStored,
    }
}

pub(crate) fn column_option(option: &sp::ColumnOption) -> a::ColumnOption {
    match option {
        sp::ColumnOption::Null => a::ColumnOption::Null,
        sp::ColumnOption::NotNull => a::ColumnOption::NotNull,
        sp::ColumnOption::Default(e) => a::ColumnOption::Default(expr(e)),
        sp::ColumnOption::PrimaryKey(k) => a::ColumnOption::PrimaryKey(primary_key(k)),
        sp::ColumnOption::Unique(u) => a::ColumnOption::Unique(unique(u)),
        sp::ColumnOption::ForeignKey(fk) => a::ColumnOption::ForeignKey(foreign_key(fk)),
        sp::ColumnOption::Check(c) => a::ColumnOption::Check(check(c)),
        sp::ColumnOption::DialectSpecific(tokens) => {
            a::ColumnOption::DialectSpecific(tokens.iter().map(ToString::to_string).collect())
        }
        sp::ColumnOption::OnUpdate(e) => a::ColumnOption::OnUpdate(expr(e)),
        sp::ColumnOption::Generated {
            generated_as: g,
            generation_expr,
            ..
        } => a::ColumnOption::Generated {
            generated_as: generated_as(*g),
            generation_expr: generation_expr.as_ref().map(expr),
        },
        sp::ColumnOption::Identity(_) => a::ColumnOption::Identity,
        other => a::ColumnOption::Other(other.to_string(), children(other).exprs),
    }
}

fn column_def(column: &sp::ColumnDef) -> a::ColumnDef {
    a::ColumnDef {
        name: ident(&column.name),
        data_type: data_type(&column.data_type),
        options: column
            .options
            .iter()
            .map(|option| a::ColumnOptionDef {
                name: option.name.as_ref().map(ident),
                option: column_option(&option.option),
            })
            .collect(),
    }
}

fn create_table(create: &sp::CreateTable) -> a::CreateTable {
    let like = |like: &sp::CreateTableLike| a::CreateTableLike {
        name: object_name(&like.name),
    };
    let mut other = Vec::new();
    other.extend(create.partition_by.as_deref().map(expr));
    other.extend(create.primary_key.as_deref().map(expr));
    a::CreateTable {
        or_replace: create.or_replace,
        temporary: create.temporary,
        if_not_exists: create.if_not_exists,
        name: object_name(&create.name),
        columns: create.columns.iter().map(column_def).collect(),
        constraints: create.constraints.iter().map(table_constraint).collect(),
        hive_distribution: match &create.hive_distribution {
            sp::HiveDistributionStyle::PARTITIONED { columns } => {
                a::HiveDistributionStyle::PARTITIONED {
                    columns: columns.iter().map(column_def).collect(),
                }
            }
            _ => a::HiveDistributionStyle::NONE,
        },
        query: create.query.as_ref().map(|q| Box::new(query(q))),
        like: create.like.as_ref().map(|kind| match kind {
            sp::CreateTableLikeKind::Parenthesized(l) => {
                a::CreateTableLikeKind::Parenthesized(like(l))
            }
            sp::CreateTableLikeKind::Plain(l) => a::CreateTableLikeKind::Plain(like(l)),
        }),
        clone: create.clone.as_ref().map(object_name),
        inherits: create
            .inherits
            .as_ref()
            .map(|parents| parents.iter().map(object_name).collect()),
        partition_of: create.partition_of.as_ref().map(object_name),
        other,
    }
}

fn alter_table_operation(operation: &sp::AlterTableOperation) -> a::AlterTableOperation {
    use a::AlterTableOperation as A;
    use sp::AlterTableOperation as S;
    match operation {
        S::AddConstraint {
            constraint,
            not_valid,
        } => A::AddConstraint {
            constraint: table_constraint(constraint),
            not_valid: *not_valid,
        },
        S::AddColumn {
            column_keyword,
            if_not_exists,
            column_def: column,
            ..
        } => A::AddColumn {
            column_keyword: *column_keyword,
            if_not_exists: *if_not_exists,
            column_def: column_def(column),
        },
        S::DropConstraint {
            if_exists, name, ..
        } => A::DropConstraint {
            if_exists: *if_exists,
            name: ident(name),
        },
        S::DropColumn {
            has_column_keyword,
            column_names,
            if_exists,
            ..
        } => A::DropColumn {
            has_column_keyword: *has_column_keyword,
            column_names: idents(column_names),
            if_exists: *if_exists,
        },
        S::DropPrimaryKey { .. } => A::DropPrimaryKey,
        S::RenameColumn {
            old_column_name,
            new_column_name,
        } => A::RenameColumn {
            old_column_name: ident(old_column_name),
            new_column_name: ident(new_column_name),
        },
        S::RenameTable { table_name } => A::RenameTable {
            table_name: match table_name {
                sp::RenameTableNameKind::As(name) => a::RenameTableNameKind::As(object_name(name)),
                sp::RenameTableNameKind::To(name) => a::RenameTableNameKind::To(object_name(name)),
            },
        },
        S::ChangeColumn {
            old_name,
            new_name,
            data_type: t,
            options,
            ..
        } => A::ChangeColumn {
            old_name: ident(old_name),
            new_name: ident(new_name),
            data_type: data_type(t),
            options: options.iter().map(column_option).collect(),
        },
        S::ModifyColumn {
            col_name,
            data_type: t,
            options,
            ..
        } => A::ModifyColumn {
            col_name: ident(col_name),
            data_type: data_type(t),
            options: options.iter().map(column_option).collect(),
        },
        S::RenameConstraint { old_name, new_name } => A::RenameConstraint {
            old_name: ident(old_name),
            new_name: ident(new_name),
        },
        S::AlterColumn { column_name, op } => A::AlterColumn {
            column_name: ident(column_name),
            op: match op {
                sp::AlterColumnOperation::SetNotNull => a::AlterColumnOperation::SetNotNull,
                sp::AlterColumnOperation::DropNotNull => a::AlterColumnOperation::DropNotNull,
                sp::AlterColumnOperation::SetDefault { value } => {
                    a::AlterColumnOperation::SetDefault { value: expr(value) }
                }
                sp::AlterColumnOperation::DropDefault => a::AlterColumnOperation::DropDefault,
                sp::AlterColumnOperation::SetDataType {
                    data_type: t,
                    using,
                    had_set,
                } => a::AlterColumnOperation::SetDataType {
                    data_type: data_type(t),
                    using: using.as_ref().map(expr),
                    had_set: *had_set,
                },
                sp::AlterColumnOperation::AddGenerated {
                    generated_as: g, ..
                } => a::AlterColumnOperation::AddGenerated {
                    generated_as: g.map(generated_as),
                },
            },
        },
        other => A::Other(other.to_string(), children(other).exprs),
    }
}

#[cfg(test)]
mod tests {
    use sqlparser::dialect::PostgreSqlDialect;
    use sqlparser::parser::Parser;

    /// Every fixture query converts into a statement that prints the same SQL.
    #[test]
    fn round_trips_fixture_queries() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
        let mut failures = Vec::new();
        let mut checked = 0;
        for pattern in ["tests/fixtures/**/*.sql", "examples/**/*.sql"] {
            for path in glob::glob(&format!("{root}{pattern}")).unwrap().flatten() {
                let Ok(sql) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(statements) = Parser::parse_sql(&PostgreSqlDialect {}, &sql) else {
                    continue;
                };
                for statement in &statements {
                    if !matches!(
                        statement,
                        sqlparser::ast::Statement::Query(_)
                            | sqlparser::ast::Statement::Insert(_)
                            | sqlparser::ast::Statement::Update(_)
                            | sqlparser::ast::Statement::Delete(_)
                    ) {
                        continue;
                    }
                    checked += 1;
                    let expected = statement.to_string();
                    let actual = super::statement(statement).to_string();
                    if expected != actual {
                        failures.push(format!(
                            "{}\n  sp: {expected}\n  a:  {actual}",
                            path.display()
                        ));
                    }
                }
            }
        }
        assert!(checked > 30, "only {checked} statements checked");
        assert!(
            failures.is_empty(),
            "{} of {checked} differ:\n{}",
            failures.len(),
            failures
                .iter()
                .take(20)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
