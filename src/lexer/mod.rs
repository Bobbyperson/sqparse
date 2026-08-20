use crate::Flavor;
use crate::lexer::token_iter::TokenIter;
use crate::token::{TerminalToken, Token, TokenType};
use std::collections::VecDeque;

mod comment;
mod error;
mod identifier;
mod literal;
mod parse_str;
mod symbol;
mod token_iter;

pub use self::error::{LexerError, LexerErrorType};

/// A token with attached metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenItem<'s> {
    /// The actual token.
    pub token: Token<'s>,

    /// The index of the corresponding closing delimiter token, if this token is an opening
    /// delimiter.
    ///
    /// # Example
    /// ```text
    /// { some other tokens }
    /// ^ open              ^ close
    /// ```
    /// In this example, the opening `{` token would have a `close_index` of 5, the index of the
    /// closing delimiter.
    pub close_index: Option<usize>,
}

/// Independently indexed token regions and lexer errors recovered from an input string.
#[derive(Debug, Clone)]
pub struct PartialTokenization<'s> {
    /// Contiguous token regions that are safe to parse independently.
    pub regions: Vec<TokenRegion<'s>>,

    /// Recovered invalid-input, unterminated-string, and delimiter errors.
    pub errors: Vec<LexerError<'s>>,
}

/// A contiguous token region with indices local to its own token vector.
#[derive(Debug, Clone)]
pub struct TokenRegion<'s> {
    /// Tokens whose opening delimiters all have matching local `close_index` values.
    pub tokens: Vec<TokenItem<'s>>,

    /// Recovery removed source immediately after this region.
    ///
    /// A parser result ending at the region boundary may only be trusted if its last token has an
    /// explicit statement boundary such as a newline or semicolon.
    pub ends_at_error: bool,
}

impl TokenRegion<'_> {
    /// Returns whether a parsed statement ending at this token index is independent of recovery.
    pub fn is_trusted_statement_end(&self, token_end: usize) -> bool {
        if token_end > self.tokens.len() {
            return false;
        }
        if !self.ends_at_error || token_end < self.tokens.len() {
            return true;
        }
        self.tokens.last().is_some_and(|item| {
            item.token.new_line.is_some()
                || matches!(
                    item.token.ty,
                    TokenType::Terminal(TerminalToken::Semicolon | TerminalToken::CloseBrace)
                )
        })
    }
}

// Returns the token that closes a tree, if the provided token is a valid opening token.
fn closing_token(opening: TokenType) -> Option<TokenType> {
    match opening {
        TokenType::Terminal(TerminalToken::OpenBrace) => {
            Some(TokenType::Terminal(TerminalToken::CloseBrace))
        }
        TokenType::Terminal(TerminalToken::OpenSquare) => {
            Some(TokenType::Terminal(TerminalToken::CloseSquare))
        }
        TokenType::Terminal(TerminalToken::OpenBracket) => {
            Some(TokenType::Terminal(TerminalToken::CloseBracket))
        }
        TokenType::Terminal(TerminalToken::OpenAttributes) => {
            Some(TokenType::Terminal(TerminalToken::CloseAttributes))
        }
        _ => None,
    }
}

// Returns true if the token is a close delimiter.
fn is_close_token(ty: TokenType) -> bool {
    matches!(
        ty,
        TokenType::Terminal(TerminalToken::CloseBrace)
            | TokenType::Terminal(TerminalToken::CloseSquare)
            | TokenType::Terminal(TerminalToken::CloseBracket)
            | TokenType::Terminal(TerminalToken::CloseAttributes)
    )
}

struct Layer<'s> {
    open_index: usize,
    close_ty: TokenType<'s>,
}

/// Parses an input string into a list of tokens.
///
/// # Example
/// ```
/// use sqparse::{Flavor, tokenize};
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
///
/// let tokens = tokenize(source, Flavor::SquirrelRespawn).unwrap();
/// assert_eq!(tokens.len(), 29);
/// ```
pub fn tokenize(val: &str, flavor: Flavor) -> Result<Vec<TokenItem<'_>>, LexerError<'_>> {
    let mut items = Vec::<TokenItem>::new();
    let mut layers = VecDeque::<Layer>::new();

    for maybe_token in TokenIter::new(val, flavor) {
        let token = maybe_token?;
        let token_index = items.len();

        // If this token is a close delimiter, it must match the innermost open delimiter.
        if is_close_token(token.ty) {
            match layers.back() {
                Some(top_layer) if top_layer.close_ty == token.ty => {
                    items[top_layer.open_index].close_index = Some(token_index);
                    layers.pop_back();
                }
                Some(top_layer) => {
                    return Err(LexerError::new(
                        LexerErrorType::MismatchedClose {
                            open: items[top_layer.open_index].token.ty,
                            expected_close: top_layer.close_ty,
                            actual_close: token.ty,
                        },
                        token.range.clone(),
                    ));
                }
                None => {
                    return Err(LexerError::new(
                        LexerErrorType::UnmatchedClose { close: token.ty },
                        token.range.clone(),
                    ));
                }
            }
        }

        // If this token is a valid opening token, push a new layer.
        if let Some(close_ty) = closing_token(token.ty) {
            layers.push_back(Layer {
                open_index: token_index,
                close_ty,
            });
        }

        items.push(TokenItem {
            token,
            close_index: None,
        });
    }

    // If there are remaining layers, there are one or more unmatched opening tokens. Otherwise
    // at this point tokenization is successful.
    match layers.back() {
        None => Ok(items),
        Some(layer) => {
            let open_token = &items[layer.open_index].token;
            Err(LexerError::new(
                LexerErrorType::UnmatchedOpener {
                    open: open_token.ty,
                    close: layer.close_ty,
                },
                open_token.range.clone(),
            ))
        }
    }
}

/// Tokenizes delimiter-balanced regions while recovering lexer errors.
///
/// No synthetic tokens are created, and every opening delimiter in every returned region has a
/// matching local [`TokenItem::close_index`]. Use [`tokenize`] when the complete source must be
/// valid.
pub fn tokenize_partial(
    val: &str,
    flavor: Flavor,
) -> Result<PartialTokenization<'_>, LexerError<'_>> {
    tokenize_partial_with_error_limit(val, flavor, usize::MAX)
}

/// Tokenizes recoverable regions while retaining at most `error_limit` lexer errors.
///
/// Recovery continues after the limit is reached, so returned regions are identical to
/// [`tokenize_partial`]. This is useful for analysis tools that cap user-facing diagnostics.
pub fn tokenize_partial_with_error_limit(
    val: &str,
    flavor: Flavor,
    error_limit: usize,
) -> Result<PartialTokenization<'_>, LexerError<'_>> {
    let mut regions = Vec::new();
    let mut errors = Vec::new();
    let mut items = Vec::<TokenItem>::new();
    let mut layers = VecDeque::<Layer>::new();
    let mut quarantine: Option<Vec<TokenType<'_>>> = None;
    let mut discard_until_boundary = false;

    for maybe_token in TokenIter::new(val, flavor) {
        let token = match maybe_token {
            Ok(token) => token,
            Err(error) => {
                push_error(&mut errors, error_limit, error);
                if quarantine.is_some() {
                    continue;
                }
                if let Some(outer_layer) = layers.front() {
                    items.truncate(outer_layer.open_index);
                    finish_region(&mut regions, &mut items, true);
                    quarantine = Some(layers.iter().map(|layer| layer.close_ty).collect());
                    layers.clear();
                } else {
                    finish_region(&mut regions, &mut items, true);
                    discard_until_boundary = true;
                }
                continue;
            }
        };

        if let Some(expected_closes) = &mut quarantine {
            if let Some(close_ty) = closing_token(token.ty) {
                expected_closes.push(close_ty);
            } else if is_close_token(token.ty) {
                unwind_quarantine(expected_closes, token.ty);
            }
            if expected_closes.is_empty() {
                quarantine = None;
            }
            continue;
        }

        if discard_until_boundary {
            if !token.before_lines.is_empty() {
                discard_until_boundary = false;
            } else {
                if token.new_line.is_some()
                    || matches!(token.ty, TokenType::Terminal(TerminalToken::Semicolon))
                {
                    discard_until_boundary = false;
                }
                continue;
            }
        }

        let token_index = items.len();
        if is_close_token(token.ty) {
            match layers.back() {
                Some(top_layer) if top_layer.close_ty == token.ty => {
                    items[top_layer.open_index].close_index = Some(token_index);
                    layers.pop_back();
                }
                Some(top_layer) => {
                    push_error(
                        &mut errors,
                        error_limit,
                        LexerError::new(
                            LexerErrorType::MismatchedClose {
                                open: items[top_layer.open_index].token.ty,
                                expected_close: top_layer.close_ty,
                                actual_close: token.ty,
                            },
                            token.range.clone(),
                        ),
                    );
                    let outer_open = layers.front().unwrap().open_index;
                    items.truncate(outer_open);
                    finish_region(&mut regions, &mut items, true);
                    let mut expected_closes = layers
                        .iter()
                        .map(|layer| layer.close_ty)
                        .collect::<Vec<_>>();
                    unwind_quarantine(&mut expected_closes, token.ty);
                    layers.clear();
                    if !expected_closes.is_empty() {
                        quarantine = Some(expected_closes);
                    }
                    continue;
                }
                None => {
                    push_error(
                        &mut errors,
                        error_limit,
                        LexerError::new(
                            LexerErrorType::UnmatchedClose { close: token.ty },
                            token.range.clone(),
                        ),
                    );
                    finish_region(&mut regions, &mut items, true);
                    continue;
                }
            }
        }

        if let Some(close_ty) = closing_token(token.ty) {
            layers.push_back(Layer {
                open_index: token_index,
                close_ty,
            });
        }
        items.push(TokenItem {
            token,
            close_index: None,
        });
    }

    if quarantine.is_none() && !layers.is_empty() {
        for layer in layers.iter().rev() {
            let open_token = &items[layer.open_index].token;
            push_error(
                &mut errors,
                error_limit,
                LexerError::new(
                    LexerErrorType::UnmatchedOpener {
                        open: open_token.ty,
                        close: layer.close_ty,
                    },
                    open_token.range.clone(),
                ),
            );
        }
        items.truncate(layers.front().unwrap().open_index);
        finish_region(&mut regions, &mut items, true);
    } else {
        finish_region(&mut regions, &mut items, false);
    }

    debug_assert!(regions.iter().all(region_has_valid_delimiters));
    Ok(PartialTokenization { regions, errors })
}

fn push_error<'s>(errors: &mut Vec<LexerError<'s>>, limit: usize, error: LexerError<'s>) {
    if errors.len() < limit {
        errors.push(error);
    }
}

fn finish_region<'s>(
    regions: &mut Vec<TokenRegion<'s>>,
    items: &mut Vec<TokenItem<'s>>,
    ends_at_error: bool,
) {
    if !items.is_empty() {
        regions.push(TokenRegion {
            tokens: std::mem::take(items),
            ends_at_error,
        });
    }
}

fn unwind_quarantine(expected_closes: &mut Vec<TokenType<'_>>, actual_close: TokenType<'_>) {
    if let Some(matching) = expected_closes
        .iter()
        .rposition(|expected| *expected == actual_close)
    {
        expected_closes.truncate(matching);
    }
}

fn region_has_valid_delimiters(region: &TokenRegion<'_>) -> bool {
    region.tokens.iter().enumerate().all(|(index, item)| {
        let Some(expected_close) = closing_token(item.token.ty) else {
            return true;
        };
        item.close_index.is_some_and(|close_index| {
            close_index > index
                && region
                    .tokens
                    .get(close_index)
                    .is_some_and(|close| close.token.ty == expected_close)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::{
        region_has_valid_delimiters, tokenize, tokenize_partial, tokenize_partial_with_error_limit,
    };
    use crate::{Flavor, LexerErrorType};

    #[test]
    fn partial_tokenization_matches_strict_tokens_for_valid_source() {
        let source = "void function Example() { Print([1, 2]) }\n";
        let strict = tokenize(source, Flavor::SquirrelRespawn).unwrap();
        let partial = tokenize_partial(source, Flavor::SquirrelRespawn).unwrap();

        assert!(partial.errors.is_empty());
        assert_eq!(partial.regions.len(), 1);
        assert_eq!(partial.regions[0].tokens, strict);
        assert!(!partial.regions[0].ends_at_error);
        assert!(region_has_valid_delimiters(&partial.regions[0]));
    }

    #[test]
    fn unmatched_close_splits_balanced_regions_and_continues() {
        let source = "void function Before() {}\n}\nvoid function After() {}\n";
        let partial = tokenize_partial(source, Flavor::SquirrelRespawn).unwrap();

        assert_eq!(partial.errors.len(), 1);
        assert!(matches!(
            partial.errors[0].ty,
            LexerErrorType::UnmatchedClose { .. }
        ));
        assert_eq!(partial.regions.len(), 2);
        assert!(partial.regions.iter().all(region_has_valid_delimiters));
    }

    #[test]
    fn mismatch_quarantines_the_outer_delimited_context() {
        let source = r#"void function Before() {}
void function Broken() { local nested = Call(] }
void function After() {}
"#;
        let partial = tokenize_partial(source, Flavor::SquirrelRespawn).unwrap();

        assert_eq!(partial.errors.len(), 1);
        assert!(matches!(
            partial.errors[0].ty,
            LexerErrorType::MismatchedClose { .. }
        ));
        assert_eq!(partial.regions.len(), 2);
        assert!(partial.regions.iter().all(region_has_valid_delimiters));
        assert!(partial.regions[0].ends_at_error);
    }

    #[test]
    fn unrelated_closers_do_not_end_delimiter_quarantine() {
        let source = r#"void function Before() {}
void function Broken() { ]
local leaked = 1
}
void function After() {}
"#;
        let partial = tokenize_partial(source, Flavor::SquirrelRespawn).unwrap();
        let retained_source = partial
            .regions
            .iter()
            .flat_map(|region| &region.tokens)
            .map(|item| &source[item.token.range.clone()])
            .collect::<String>();

        assert!(!retained_source.contains("leaked"));
        assert!(retained_source.contains("Before"));
        assert!(retained_source.contains("After"));
    }

    #[test]
    fn unrelated_closers_do_not_end_raw_error_quarantine() {
        let source = r#"void function Before() {}
void function Broken() { €
)
local leaked = 1
}
void function After() {}
"#;
        let partial = tokenize_partial(source, Flavor::SquirrelRespawn).unwrap();
        let retained_source = partial
            .regions
            .iter()
            .flat_map(|region| &region.tokens)
            .map(|item| &source[item.token.range.clone()])
            .collect::<String>();

        assert!(!retained_source.contains("leaked"));
        assert!(retained_source.contains("Before"));
        assert!(retained_source.contains("After"));
    }

    #[test]
    fn unmatched_openers_are_never_exposed_to_the_parser() {
        let source = "void function Before() {}\nvoid function Broken() { Call(\n";
        let partial = tokenize_partial(source, Flavor::SquirrelRespawn).unwrap();

        assert_eq!(partial.errors.len(), 2);
        assert!(
            partial
                .errors
                .iter()
                .all(|error| matches!(error.ty, LexerErrorType::UnmatchedOpener { .. }))
        );
        assert_eq!(partial.regions.len(), 1);
        assert!(partial.regions[0].ends_at_error);
        assert!(region_has_valid_delimiters(&partial.regions[0]));
    }

    #[test]
    fn truncated_prefix_does_not_create_a_trusted_forward_declaration() {
        let partial =
            tokenize_partial("global function FalsePositive {", Flavor::SquirrelRespawn).unwrap();
        let region = &partial.regions[0];
        let parsed = crate::parse_partial(&region.tokens, Flavor::SquirrelRespawn);

        assert_eq!(parsed.statements.len(), 1);
        assert!(!region.is_trusted_statement_end(parsed.statements[0].token_range.end));
    }

    #[test]
    fn collects_independent_unmatched_closes() {
        let partial = tokenize_partial("}\n]\n", Flavor::SquirrelRespawn).unwrap();

        assert_eq!(partial.errors.len(), 2);
        assert!(partial.regions.is_empty());
    }

    #[test]
    fn recovers_multiple_invalid_inputs_and_preserves_byte_ranges() {
        let source = "void function Before() {}\n€ bad\n£\nvoid function After() {}\n";
        let partial = tokenize_partial(source, Flavor::SquirrelRespawn).unwrap();

        assert_eq!(partial.errors.len(), 2);
        assert!(
            partial
                .errors
                .iter()
                .all(|error| matches!(error.ty, LexerErrorType::InvalidInput))
        );
        assert_eq!(partial.errors[0].range, 26..29);
        assert_eq!(partial.errors[1].range, 34..36);
        assert_eq!(partial.regions.len(), 2);
        assert!(partial.regions.iter().all(region_has_valid_delimiters));
    }

    #[test]
    fn limits_errors_without_stopping_region_recovery() {
        let source = "€\n£\n¥\nvoid function After() {}\n";
        let partial =
            tokenize_partial_with_error_limit(source, Flavor::SquirrelRespawn, 2).unwrap();
        let retained_source = partial
            .regions
            .iter()
            .flat_map(|region| &region.tokens)
            .map(|item| &source[item.token.range.clone()])
            .collect::<String>();

        assert_eq!(partial.errors.len(), 2);
        assert!(retained_source.contains("After"));
    }

    #[test]
    fn invalid_input_discards_the_rest_of_its_statement() {
        let source = "€ global function FalsePositive\nvoid function After() {}\n";
        let partial = tokenize_partial(source, Flavor::SquirrelRespawn).unwrap();
        let retained_source = partial
            .regions
            .iter()
            .flat_map(|region| &region.tokens)
            .map(|item| &source[item.token.range.clone()])
            .collect::<String>();

        assert_eq!(partial.errors.len(), 1);
        assert!(!retained_source.contains("FalsePositive"));
        assert!(retained_source.contains("After"));
    }

    #[test]
    fn recovers_after_an_unterminated_line_string() {
        let source = "void function Before() {}\nlocal broken = \"text\nvoid function After() {}\n";
        let partial = tokenize_partial(source, Flavor::SquirrelRespawn).unwrap();

        assert_eq!(partial.errors.len(), 1);
        assert!(matches!(
            partial.errors[0].ty,
            LexerErrorType::EndOfLineInsideString
        ));
        assert_eq!(partial.regions.len(), 2);
        assert!(partial.regions.iter().all(region_has_valid_delimiters));
    }

    #[test]
    fn recovers_before_an_unterminated_multiline_verbatim_string() {
        let source = "void function Before() {}\nlocal broken = @\"first\nsecond\n";
        let partial = tokenize_partial(source, Flavor::SquirrelRespawn).unwrap();

        assert_eq!(partial.errors.len(), 1);
        assert!(matches!(
            partial.errors[0].ty,
            LexerErrorType::EndOfInputInsideString
        ));
        assert_eq!(partial.regions.len(), 1);
        assert!(region_has_valid_delimiters(&partial.regions[0]));
    }
}
