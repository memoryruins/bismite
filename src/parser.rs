use crate::{
    ast,
    token::{Token, TokenKind},
};
use codespan::ByteSpan;
use lazy_static::lazy_static;
use logos::{Lexer, Logos};
use std::{
    collections::VecDeque,
    convert::{TryFrom, TryInto},
    sync::RwLock,
};
use string_interner::{DefaultStringInterner, Sym};

lazy_static! {
    pub static ref GLOBAL_INTERNER: RwLock<DefaultStringInterner> =
        RwLock::new(DefaultStringInterner::new());
}

pub struct Parser<'source> {
    lexer: Lexer<TokenKind, &'source str>,
    peek: Option<Token<'source>>,
}

type ParseResult<'a, T> = Result<T, ParserError<'a>>;

impl<'parser, 'source: 'parser> Parser<'source> {
    pub fn new(input: &'source str) -> Parser<'source> {
        let mut lexer = TokenKind::lexer(input);
        let token = Token::new(lexer.token, lexer.slice(), lexer.range());
        lexer.advance();

        Self {
            lexer,
            peek: Some(token),
        }
    }

    pub fn parse(&'parser mut self) -> ParseResult<'source, Vec<ast::Decls>> {
        let mut decls = Vec::new();

        loop {
            let peek = self.peek();

            if let Err(ParserError::InvalidToken(
                Token {
                    kind: TokenKind::Eof,
                    ..
                },
                _,
            )) = peek
            {
                break;
            }

            let peek = peek?;

            match peek.kind {
                TokenKind::Struct => decls.push(ast::Decls::Struct(self.parse_struct()?)),
                TokenKind::Fn => decls.push(ast::Decls::Fn(self.parse_fn()?)),
                _ => Err(peek)?,
            }
        }

        Ok(decls)
    }

    fn parse_fn(&'parser mut self) -> ParseResult<'source, ast::FnDecl> {
        let fn_token = self.eat(TokenKind::Fn)?;
        let ident = self.parse_ident()?;
        self.eat(TokenKind::LParen)?;

        let mut parameters = Vec::new();

        while self.peek()?.kind != TokenKind::RParen {
            parameters.push(self.parse_named_parameter()?);

            if self.peek()?.kind == TokenKind::Comma {
                self.eat(TokenKind::Comma)?;
            }
        }

        self.eat(TokenKind::RParen)?;

        let ret = if self.peek()?.kind == TokenKind::Arrow {
            self.eat(TokenKind::Arrow)?;
            Some(self.parse_type()?)
        } else {
            None
        };

        self.eat(TokenKind::LBrace)?;

        let mut statements = Vec::new();

        while self.peek()?.kind != TokenKind::RBrace {
            statements.push(self.parse_statement()?);
        }

        let rb = self.eat(TokenKind::RBrace)?;

        Ok(ast::FnDecl::new(
            ident,
            parameters,
            ret,
            statements,
            ByteSpan::new(fn_token.span.start(), rb.span.end()),
        ))
    }

    fn parse_named_parameter(&'parser mut self) -> ParseResult<'source, ast::ParameterDecl> {
        let ident = self.parse_ident()?;
        self.eat(TokenKind::Colon)?;
        let ty = self.parse_type()?;

        let (start, end) = (ident.span.start(), ty.span.end());

        Ok(ast::ParameterDecl::new(
            ident,
            ty,
            ByteSpan::new(start, end),
        ))
    }

    fn parse_statement(&'parser mut self) -> ParseResult<'source, ast::StatementDecl> {
        match self.peek()?.kind {
            TokenKind::Let => self.parse_variable_decl(),
            _ => unimplemented!(),
        }
    }

    fn parse_variable_decl(&'parser mut self) -> ParseResult<'source, ast::StatementDecl> {
        let l = self.eat(TokenKind::Let)?;
        let ident = self.parse_ident()?;

        let ty = if self.peek()?.kind == TokenKind::Colon {
            self.eat(TokenKind::Colon)?;
            Some(self.parse_type()?)
        } else {
            None
        };

        self.eat(TokenKind::Assign)?;

        let expr = self.parse_expression()?;
        let s = self.eat(TokenKind::Semicolon)?;

        Ok(ast::StatementDecl::VarDecl(ast::VarDecl::new(
            ident,
            ty,
            expr,
            ByteSpan::new(l.span.start(), s.span.end()),
        )))
    }

    fn parse_struct(&'parser mut self) -> ParseResult<'source, ast::StructDecl> {
        let span_begin = self.eat(TokenKind::Struct)?.span;
        let ident = self.parse_ident()?;

        self.eat(TokenKind::LBrace)?;

        let fields = match self.peek()? {
            Token {
                kind: TokenKind::Ident,
                ..
            } => self.parse_fields()?,
            tkn => Err(tkn)?,
        };

        let end_span = self.eat(TokenKind::RBrace)?.span;

        Ok(ast::StructDecl {
            ident,
            fields,
            span: ByteSpan::new(span_begin.start(), end_span.end()),
        })
    }

    fn parse_fields(&'parser mut self) -> ParseResult<'source, Vec<ast::FieldDecl>> {
        let mut fields = Vec::new();

        loop {
            let peek = self.peek()?;

            match peek.kind {
                TokenKind::Ident => {
                    let ident = self.parse_ident()?;
                    self.eat(TokenKind::Colon)?;
                    let field_type = self.parse_type()?;

                    if let TokenKind::Comma = self.peek()?.kind {
                        self.eat(TokenKind::Comma)?;
                    }

                    let end = field_type.span.end();

                    fields.push(ast::FieldDecl {
                        ident,
                        field_type,
                        span: ByteSpan::new(ident.span.start(), end),
                    })
                }
                TokenKind::RBrace => break,
                _ => return Err(peek)?,
            }
        }

        Ok(fields)
    }

    fn parse_ident(&'parser mut self) -> ParseResult<'source, ast::Ident> {
        let tkn = self.eat(TokenKind::Ident)?;

        let sym = GLOBAL_INTERNER.write().unwrap().get_or_intern(tkn.lit);

        Ok(ast::Ident {
            span: tkn.span,
            id: sym,
        })
    }

    fn parse_type(&'parser mut self) -> ParseResult<'source, ast::Type> {
        let peek = self.peek()?;

        let kind = match peek.kind {
            TokenKind::Ident | TokenKind::PathSeparator => {
                let path = self.parse_path()?;

                ast::TypeKind::Path(path)
            }
            TokenKind::LBracket => self.parse_array_ty()?,
            _ => Err(peek)?,
        };

        Ok(ast::Type {
            span: ByteSpan::new(peek.span.start(), self.peek()?.span.end()),
            kind,
        })
    }

    fn parse_array_ty(&'parser mut self) -> ParseResult<'source, ast::TypeKind> {
        self.eat(TokenKind::LBracket)?;
        let ty = self.parse_type()?;
        self.eat(TokenKind::Semicolon)?;
        let len = self.parse_integer_literal()?;

        if len.is_negative() {
            Err(ParserError::InvalidArraySize(len))?;
        }

        Ok(ast::TypeKind::Array(Box::new(ty), len as usize))
    }

    fn parse_path(&'parser mut self) -> ParseResult<'source, ast::Path> {
        let peek = self.peek()?;
        let mut segments = Vec::new();

        if peek.kind == TokenKind::PathSeparator {
            self.eat(TokenKind::PathSeparator)?;
        }

        while self.peek()?.kind == TokenKind::Ident {
            segments.push(self.parse_path_segment()?);
        }

        let span_start = segments[0].ident.span.start();
        let span_end = segments[segments.len() - 1].ident.span.end();

        Ok(ast::Path {
            span: ByteSpan::new(span_start, span_end),
            segments,
        })
    }

    fn parse_path_segment(&'parser mut self) -> ParseResult<'source, ast::PathSegment> {
        Ok(ast::PathSegment {
            ident: self.parse_ident()?,
        })
    }

    fn parse_array_literal(&'parser mut self) -> ParseResult<'source, ast::Literal> {
        let start = self.eat(TokenKind::LBracket)?.span.start();
        let mut items = Vec::new();

        while self.peek()?.kind != TokenKind::RBracket {
            items.push(self.parse_expression()?);

            if self.peek()?.kind != TokenKind::RBracket {
                self.eat(TokenKind::Comma)?;
            }
        }

        let end = self.eat(TokenKind::RBracket)?.span.end();

        Ok(ast::Literal::new(
            ast::LiteralKind::Array(items),
            ByteSpan::new(start, end),
        ))
    }

    fn parse_integer_literal(&'parser mut self) -> ParseResult<'source, i128> {
        let lit = self.parse_literal()?;

        match lit.kind {
            ast::LiteralKind::Int(val) => Ok(val),
            _ => Err(ParserError::ExpectedIntegerLit(lit.kind)),
        }
    }

    fn parse_literal(&'parser mut self) -> ParseResult<'source, ast::Literal> {
        use std::str::FromStr;

        let peek = self.peek()?;

        match peek.kind {
            TokenKind::DecLit => Ok(ast::Literal::new(
                ast::LiteralKind::Int(
                    i128::from_str_radix(self.eat(TokenKind::DecLit)?.lit, 10).unwrap(),
                ),
                peek.span,
            )),
            TokenKind::HexLit => Ok(ast::Literal::new(
                ast::LiteralKind::Int(
                    i128::from_str_radix(&self.eat(TokenKind::HexLit)?.lit[2..], 16).unwrap(),
                ),
                peek.span,
            )),
            TokenKind::OctLit => Ok(ast::Literal::new(
                ast::LiteralKind::Int(
                    i128::from_str_radix(&self.eat(TokenKind::OctLit)?.lit[2..], 8).unwrap(),
                ),
                peek.span,
            )),
            TokenKind::BinLit => Ok(ast::Literal::new(
                ast::LiteralKind::Int(
                    i128::from_str_radix(&self.eat(TokenKind::BinLit)?.lit[2..], 2).unwrap(),
                ),
                peek.span,
            )),
            TokenKind::FloatLit => Ok(ast::Literal::new(
                ast::LiteralKind::Float(f64::from_str(self.eat(TokenKind::FloatLit)?.lit).unwrap()),
                peek.span,
            )),
            TokenKind::RawStr => Ok(ast::Literal::new(
                ast::LiteralKind::RawStr({
                    let tkn = self.eat(TokenKind::RawStr)?;
                    let len = tkn.lit.len();

                    GLOBAL_INTERNER
                        .write()
                        .unwrap()
                        .get_or_intern(&tkn.lit[1..len - 1])
                }),
                peek.span,
            )),
            TokenKind::LBracket => Ok(self.parse_array_literal()?),
            TokenKind::Ident => {
                let ident = self.parse_ident()?;
                Ok(ast::Literal::new(ident.into(), ident.span))
            }
            _ => Err(ParserError::UnexpectedToken(self.peek()?)),
        }
    }

    fn parse_expression(&'parser mut self) -> ParseResult<'source, ast::Expression> {
        let prim = self.parse_primary()?;
        self.parse_inner_expression(prim, 0)
    }

    fn parse_inner_expression(
        &'parser mut self,
        mut lhs: ast::Expression,
        min_prec: u8,
    ) -> ParseResult<'source, ast::Expression> {
        let mut peek = self.peek()?;
        let continue_loop = |token| match ast::BinaryOp::try_from(token) {
            Ok(op) if op.precedence() >= min_prec => (true, op.precedence()),
            _ => (false, 0),
        };

        while continue_loop(peek).0 {
            let op = ast::BinaryOp::try_from(self.next()?).unwrap();
            let mut rhs = self.parse_primary()?;
            peek = self.peek()?;

            while let (true, prec) = match ast::BinaryOp::try_from(peek) {
                Ok(op2) if op2.precedence() > op.precedence() => (true, op2.precedence()),
                _ => (false, 0),
            } {
                rhs = self.parse_inner_expression(rhs, prec)?;
                peek = self.peek()?;
            }

            let lhs_span = lhs.span.start();
            let rhs_span = rhs.span.end();

            if op == ast::BinaryOp::Access {
                if let ast::ExpressionKind::Literal(ast::Literal {
                    kind: ast::LiteralKind::Ident(ident),
                    ..
                }) = rhs.kind
                {
                    lhs = ast::Expression::new(
                        ast::ExpressionKind::FieldAccess(Box::new(lhs), ident),
                        ByteSpan::new(lhs_span, rhs_span),
                    );
                } else if let ast::ExpressionKind::FnCall(mut segment, mut exprs) = rhs.kind {
                    lhs = ast::Expression::new(
                        ast::ExpressionKind::MethodCall(segment.segments.remove(0), {
                            exprs.insert(0, lhs);
                            exprs
                        }),
                        ByteSpan::new(lhs_span, rhs_span),
                    );
                } else {
                    panic!("error here");
                }
            } else {
                lhs = ast::Expression::new(
                    ast::ExpressionKind::Binary(Box::new(lhs), op, Box::new(rhs)),
                    ByteSpan::new(lhs_span, rhs_span),
                );
            }
        }

        Ok(lhs)
    }

    fn parse_primary(&'parser mut self) -> ParseResult<'source, ast::Expression> {
        let peek = self.peek()?;

        match peek.kind {
            TokenKind::Ident => {
                let ident = self.parse_ident()?;

                match self.peek()?.kind {
                    TokenKind::LParen => {
                        self.eat(TokenKind::LParen)?;
                        let list = self.parse_expr_list(TokenKind::RParen)?;
                        let end = self.eat(TokenKind::RParen)?.span.end();

                        Ok(ast::Expression::new(
                            ast::ExpressionKind::FnCall(
                                ast::Path::new(vec![ast::PathSegment { ident }], ident.span),
                                list,
                            ),
                            ByteSpan::new(ident.span.start(), end),
                        ))
                    }
                    _ => Ok(ast::Expression::new(
                        ast::ExpressionKind::Literal(ast::Literal::new(
                            ast::LiteralKind::Ident(ident),
                            ident.span,
                        )),
                        ident.span,
                    )),
                }
            }
            /*TokenKind::PathSeparator => {
                let path = self.parse_path()?;
                let path_start = path.span.start();
                let path_end = path.span.end();
            }*/
            TokenKind::LBracket
            | TokenKind::DecLit
            | TokenKind::HexLit
            | TokenKind::OctLit
            | TokenKind::BinLit
            | TokenKind::FloatLit
            | TokenKind::RawStr => {
                let lit = self.parse_literal()?;
                let lit_span = lit.span;

                Ok(ast::Expression::new(
                    ast::ExpressionKind::Literal(lit),
                    lit_span,
                ))
            }
            TokenKind::Minus | TokenKind::Not => {
                let uo_t = self.next()?;
                let uo = uo_t.try_into().unwrap();
                let rhs = self.parse_expression()?;
                let rhs_end = rhs.span.end();
                Ok(ast::Expression::new(
                    ast::ExpressionKind::Unary(uo, Box::new(rhs)),
                    ByteSpan::new(uo_t.span.start(), rhs_end),
                ))
            }
            TokenKind::LParen => {
                self.next()?;
                let expr = self.parse_expression()?;
                self.eat(TokenKind::RParen)?;

                Ok(expr)
            }
            _ => unimplemented!(),
        }
    }

    fn parse_expr_list(
        &'parser mut self,
        closing: TokenKind,
    ) -> ParseResult<'source, Vec<ast::Expression>> {
        let mut exprs = Vec::new();

        loop {
            exprs.push(self.parse_expression()?);

            let peek = self.peek()?;

            if peek.kind == TokenKind::Comma {
                self.eat(TokenKind::Comma)?;

                if self.peek()?.kind == closing {
                    break;
                }
            } else if peek.kind == closing {
                break;
            }
        }

        Ok(exprs)
    }

    fn parse_field_or_method(
        &'parser mut self,
        expr: ast::Expression,
    ) -> ParseResult<'source, ast::Expression> {
        let expr_start = expr.span.start();
        let ident = self.parse_ident()?;

        if self.peek()?.kind == TokenKind::LParen {
            self.eat(TokenKind::LParen)?;
            let exprs = self.parse_expr_list(TokenKind::RParen)?;
            let rp = self.eat(TokenKind::RParen)?;
            Ok(ast::Expression::new(
                ast::ExpressionKind::MethodCall(
                    if let ast::Expression {
                        kind: ast::ExpressionKind::Path(path),
                        ..
                    } = &expr
                    {
                        path.segments.last().unwrap().clone()
                    } else {
                        panic!("Start of method call was not a path")
                    },
                    exprs,
                ),
                ByteSpan::new(expr_start, rp.span.end()),
            ))
        } else {
            Ok(ast::Expression::new(
                ast::ExpressionKind::FieldAccess(Box::new(expr), ident),
                ByteSpan::new(expr_start, ident.span.end()),
            ))
        }
    }

    fn eat(&'parser mut self, expected: TokenKind) -> ParseResult<'source, Token<'source>> {
        let tkn = self.next()?;

        if tkn.kind == expected {
            Ok(tkn)
        } else {
            Err(tkn)?
        }
    }

    fn eat_one_of(
        &'parser mut self,
        expecteds: &[TokenKind],
    ) -> ParseResult<'source, Token<'source>> {
        let tkn = self.next()?;

        for &kind in expecteds {
            if tkn.kind == kind {
                return Ok(tkn);
            }
        }

        Err(tkn)?
    }

    fn eat_optional(
        &'parser mut self,
        expected: TokenKind,
    ) -> ParseResult<'source, Option<Token<'source>>> {
        let tkn = self.peek()?;

        if tkn.kind == expected {
            Ok(Some(self.next()?))
        } else {
            Ok(None)
        }
    }

    fn next(&'parser mut self) -> ParseResult<'source, Token<'source>> {
        if let Some(tkn) = self.peek {
            self.peek = None;
            Ok(tkn)
        } else {
            let tkn = Token::new(self.lexer.token, self.lexer.slice(), self.lexer.range());

            let ret = match &tkn.kind {
                TokenKind::Error | TokenKind::Eof => Err(tkn)?,
                _ => Ok(tkn),
            };

            self.lexer.advance();

            ret
        }
    }

    fn peek(&'parser mut self) -> ParseResult<'source, Token<'source>> {
        if let Some(tkn) = self.peek {
            Ok(tkn)
        } else {
            let tkn = self.next()?;
            self.peek = Some(tkn);
            Ok(tkn)
        }
    }
}

#[test]
fn aaaaaaa() {
    let mut parser = Parser::new("(10 + 5 - (2 * 3 / (3 ^ 8)) == 7) != 1;");

    println!("{:?}", parser.parse_expression());
    panic!();
}

#[test]
fn aaaaaaa2() {
    let mut parser = Parser::new(r#"[0, 1, 2, 3] + "foo bar" - -52.0;"#);

    println!("{:?}", parser.parse_expression());
    panic!();
}

/// Represents a parser error.
#[derive(Debug)]
pub enum ParserError<'src> {
    ExpectedIntegerLit(ast::LiteralKind),
    InvalidToken(Token<'src>, Option<TokenKind>),
    InvalidArraySize(i128),
    UnexpectedToken(Token<'src>),
}

impl<'src> From<Token<'src>> for ParserError<'src> {
    fn from(token: Token<'src>) -> ParserError<'src> {
        ParserError::InvalidToken(token, None)
    }
}

impl<'src> ::std::fmt::Display for ParserError<'src> {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_test() {}
}
