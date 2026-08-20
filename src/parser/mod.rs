mod array;
mod class;
mod context;
mod control;
mod enum_;
mod error;
mod expression;
mod function;
mod global;
mod identifier;
mod operator;
mod parse_result_ext;
mod slot;
mod statement;
mod struct_;
mod table;
mod token_list;
mod token_list_ext;
mod type_;
mod variable;

pub use self::context::ContextType;
pub use self::error::{ParseError, ParseErrorContext, ParseErrorType, TokenAffinity};
use crate::Flavor;
use crate::ast::{Program, Statement};

use crate::lexer::TokenItem;
use crate::parser::statement::statement;
use crate::parser::token_list::TokenList;
use crate::parser::token_list_ext::TokenListExt;
use crate::token::{TerminalToken, TokenType};
use std::ops::Range;

type ParseResult<'s, T> = Result<(TokenList<'s>, T), ParseError>;

/// Parses an input token list into a syntax tree.
///
/// # Example
/// ```
/// use sqparse::{Flavor, parse, tokenize};
///
/// let source = r#"
/// global function MyFunction
///
/// struct {
///     int a
/// } file
///
/// string function MyFunction( List<number> values ) {
///     values.push(1 + 2)
/// }
/// "#;
/// let tokens = tokenize(source, Flavor::SquirrelRespawn).unwrap();
///
/// let program = parse(&tokens, Flavor::SquirrelRespawn).unwrap();
/// assert_eq!(program.statements.len(), 3);
/// ```
pub fn parse<'s>(items: &'s [TokenItem<'s>], flavor: Flavor) -> Result<Program<'s>, ParseError> {
    let tokens = TokenList::new(flavor, items);
    let (tokens, statements) = tokens.many_until_ended(statement)?;
    assert!(tokens.is_ended());
    Ok(Program { statements })
}

/// Statements and errors recovered from a token list that may contain parser errors.
///
/// Every returned statement was parsed normally from the original tokens. Malformed top-level
/// regions are omitted rather than represented by synthetic AST nodes. Tokenization must still
/// succeed, so this does not recover from unmatched delimiters or lexer errors.
#[derive(Debug, Clone)]
pub struct PartialParse<'s> {
    /// Independently valid top-level statements.
    pub statements: Vec<ParsedStatement<'s>>,

    /// Parser errors and the token regions discarded to resume parsing.
    pub errors: Vec<ParseRecovery>,
}

/// A normally parsed statement and the absolute token range it consumed.
#[derive(Debug, Clone)]
pub struct ParsedStatement<'s> {
    /// The parsed statement.
    pub statement: Statement<'s>,

    /// Absolute token indices consumed by the statement.
    pub token_range: Range<usize>,
}

/// A parser error and the top-level token region discarded after it.
#[derive(Debug, Clone)]
pub struct ParseRecovery {
    /// The original parser error.
    pub error: ParseError,

    /// Absolute token indices discarded before parsing resumed.
    pub token_range: Range<usize>,
}

/// Parses all independently valid top-level statements, skipping malformed regions.
///
/// This is intended for analysis tools that can use an incomplete syntax tree. Formatters and
/// other source-preserving transformations should continue to use [`parse`].
pub fn parse_partial<'s>(items: &'s [TokenItem<'s>], flavor: Flavor) -> PartialParse<'s> {
    let mut tokens = TokenList::new(flavor, items);
    let mut statements = Vec::new();
    let mut errors = Vec::new();

    while !tokens.is_ended() {
        let statement_start = tokens.start_index();
        match statement(tokens) {
            Ok((next, parsed)) => {
                statements.push(ParsedStatement {
                    statement: parsed,
                    token_range: statement_start..next.start_index(),
                });
                tokens = next;
            }
            Err(error) => {
                let recovery_end = recovery_end(items, statement_start, &error);
                errors.push(ParseRecovery {
                    error,
                    token_range: statement_start..recovery_end,
                });
                tokens = TokenList::at(flavor, items, recovery_end);
            }
        }
    }

    PartialParse { statements, errors }
}

fn recovery_end(items: &[TokenItem<'_>], start: usize, error: &ParseError) -> usize {
    if error.token_index > start
        && error.token_index <= items.len()
        && items[error.token_index - 1].token.new_line.is_some()
        && is_top_level_index(items, start, error.token_index)
    {
        return error.token_index;
    }

    let error_end = if error.token_index < items.len() {
        error.token_index + 1
    } else {
        items.len()
    };
    let mut index = start;
    while index < items.len() {
        let consumed = if let Some(close_index) = items[index].close_index {
            index = close_index + 1;
            close_index
        } else {
            let consumed = index;
            index += 1;
            consumed
        };
        if index >= error_end && is_recovery_boundary(&items[consumed]) {
            return index;
        }
    }
    items.len()
}

fn is_top_level_index(items: &[TokenItem<'_>], start: usize, target: usize) -> bool {
    let mut index = start;
    while index < target {
        if let Some(close_index) = items[index].close_index {
            if close_index >= target {
                return false;
            }
            index = close_index + 1;
        } else {
            index += 1;
        }
    }
    true
}

fn is_recovery_boundary(item: &TokenItem<'_>) -> bool {
    item.token.new_line.is_some()
        || matches!(
            item.token.ty,
            TokenType::Terminal(TerminalToken::Semicolon | TerminalToken::CloseBrace)
        )
}

#[cfg(test)]
mod tests {
    use super::{parse, parse_partial};
    use crate::ast::StatementType;
    use crate::{Flavor, tokenize};

    fn function_names(partial: &super::PartialParse<'_>) -> Vec<String> {
        partial
            .statements
            .iter()
            .filter_map(|statement| match &statement.statement.ty {
                StatementType::FunctionDefinition(function) => {
                    Some(function.name.last_item.value.to_string())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn partial_parse_matches_strict_parse_for_valid_source() {
        let source = "void function First() {}\nvoid function Second() {}\n";
        let tokens = tokenize(source, Flavor::SquirrelRespawn).unwrap();
        let strict = parse(&tokens, Flavor::SquirrelRespawn).unwrap();
        let partial = parse_partial(&tokens, Flavor::SquirrelRespawn);

        assert_eq!(partial.statements.len(), strict.statements.len());
        assert!(partial.errors.is_empty());
    }

    #[test]
    fn recovers_valid_statements_around_a_malformed_function() {
        let source = r#"void function Before() {}
void function Broken() { local value = }
void function After() {}
"#;
        let tokens = tokenize(source, Flavor::SquirrelRespawn).unwrap();
        let partial = parse_partial(&tokens, Flavor::SquirrelRespawn);

        assert_eq!(function_names(&partial), ["Before", "After"]);
        assert_eq!(partial.errors.len(), 1);
        assert!(partial.errors[0].token_range.start < partial.errors[0].token_range.end);
    }

    #[test]
    fn recovers_multiple_malformed_lines_without_consuming_the_next_statement() {
        let source = "local first =\nlocal second =\nlocal valid = 1\n";
        let tokens = tokenize(source, Flavor::SquirrelRespawn).unwrap();
        let partial = parse_partial(&tokens, Flavor::SquirrelRespawn);

        assert_eq!(partial.statements.len(), 1);
        assert_eq!(partial.errors.len(), 2);
    }

    #[test]
    fn does_not_leak_nested_statements_from_a_malformed_function() {
        let source = r#"void function Broken() {
	local nested = 1
	local invalid =
}
local topLevel = 2
"#;
        let tokens = tokenize(source, Flavor::SquirrelRespawn).unwrap();
        let partial = parse_partial(&tokens, Flavor::SquirrelRespawn);

        assert_eq!(partial.statements.len(), 1);
        assert_eq!(partial.errors.len(), 1);
        assert!(matches!(
            partial.statements[0].statement.ty,
            StatementType::VarDefinition(_)
        ));
    }

    #[test]
    fn recovers_at_a_same_line_semicolon() {
        let source = "local invalid = ; local valid = 1\n";
        let tokens = tokenize(source, Flavor::SquirrelRespawn).unwrap();
        let partial = parse_partial(&tokens, Flavor::SquirrelRespawn);

        assert_eq!(partial.statements.len(), 1);
        assert_eq!(partial.errors.len(), 1);
        assert_eq!(partial.errors[0].token_range.end, 4);
    }

    #[test]
    fn strict_parse_still_returns_the_first_error() {
        let source = "local first =\nlocal second =\n";
        let tokens = tokenize(source, Flavor::SquirrelRespawn).unwrap();
        let strict_error = parse(&tokens, Flavor::SquirrelRespawn).unwrap_err();
        let partial = parse_partial(&tokens, Flavor::SquirrelRespawn);

        assert_eq!(
            partial.errors[0].error.token_index,
            strict_error.token_index
        );
        assert_eq!(partial.errors[0].error.ty, strict_error.ty);
    }
}
