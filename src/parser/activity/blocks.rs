//! Block-construct sub-parsers (`if`/`while`/`repeat`/`fork`/`split`/
//! `switch`/`partition`). Each recurses into `parse_stmts` up to its
//! matching terminator.

use crate::diagnostics::Result;
use crate::ir::{ActionKind, ActivityStmt, ElseIfBranch, PartitionKind, SwitchCase};

use super::scan::*;
use super::{Parser, Terminator};

impl<'a> Parser<'a> {
    pub(super) fn parse_if(&mut self, line_no: usize) -> Result<ActivityStmt> {
        let raw = self.lines[self.pos].text.trim().to_string();
        self.pos += 1;
        let (cond, then_label) = parse_if_head(&raw);

        let (then_branch, term) = self.parse_stmts(Terminator::EndIf)?;
        let mut elseifs: Vec<ElseIfBranch> = Vec::new();
        let mut else_label: Option<String> = None;
        let mut else_branch: Option<Vec<ActivityStmt>> = None;
        let mut hit = term;

        loop {
            match hit.kind {
                Terminator::ElseIf => {
                    let (cond, label) = parse_elseif_head(&hit.raw);
                    let (br, next) = self.parse_stmts(Terminator::EndIf)?;
                    elseifs.push(ElseIfBranch {
                        cond,
                        label,
                        branch: br,
                        line: hit.line,
                    });
                    hit = next;
                }
                Terminator::Else => {
                    else_label = parse_else_label(&hit.raw);
                    let (br, next) = self.parse_stmts(Terminator::EndIf)?;
                    else_branch = Some(br);
                    hit = next;
                }
                Terminator::EndIf | Terminator::Eof => break,
                _ => break,
            }
        }

        Ok(ActivityStmt::If {
            cond,
            then_label,
            then_branch,
            elseifs,
            else_label,
            else_branch,
            line: line_no,
        })
    }

    pub(super) fn parse_while(&mut self, line_no: usize) -> Result<ActivityStmt> {
        let raw = self.lines[self.pos].text.trim().to_string();
        self.pos += 1;
        let (cond, is_label) = parse_while_head(&raw);
        let (body, term) = self.parse_stmts(Terminator::EndWhile)?;
        let not_label = parse_endwhile_label(&term.raw);
        Ok(ActivityStmt::While {
            cond,
            is_label,
            not_label,
            body,
            line: line_no,
        })
    }

    pub(super) fn parse_repeat(&mut self, line_no: usize) -> Result<ActivityStmt> {
        let raw = self.lines[self.pos].text.trim().to_string();
        self.pos += 1;
        // First-action sugar: `repeat :foo;` opens the loop and emits
        // `:foo;` as the first body statement. We synthesise that.
        let mut body: Vec<ActivityStmt> = Vec::new();
        if let Some(label_with_semi) = raw.strip_prefix("repeat :") {
            let label = label_with_semi.trim_end_matches(';').to_string();
            body.push(ActivityStmt::Action {
                label: vec![label],
                kind: ActionKind::Rectangle,
                color: None,
                url: None,
                notes: Vec::new(),
                edge_label: self.pending_arrow.take(),
                line: line_no,
            });
        }

        // Gather body until `repeat while (…)` arrives. We allow a
        // `backward :label;` directive inside the loop — sniff it as we
        // pull statements.
        let (mut more_body, term) = self.parse_stmts(Terminator::RepeatWhile)?;
        body.append(&mut more_body);
        let backward = take_backward(&mut body);

        let (cond, is_label, not_label) = parse_repeat_while_head(&term.raw);
        Ok(ActivityStmt::Repeat {
            body,
            backward,
            cond,
            is_label,
            not_label,
            line: line_no,
        })
    }

    pub(super) fn parse_fork(&mut self, line_no: usize) -> Result<ActivityStmt> {
        // Consume the `fork` opener already at self.pos.
        self.pos += 1;
        let mut branches: Vec<Vec<ActivityStmt>> = Vec::new();
        let mut merge = true;
        loop {
            let (br, term) = self.parse_stmts(Terminator::EndFork)?;
            branches.push(br);
            match term.kind {
                Terminator::ForkAgain => continue,
                Terminator::EndFork => {
                    merge = parse_fork_end_merge(&term.raw);
                    break;
                }
                _ => break,
            }
        }
        Ok(ActivityStmt::Fork {
            branches,
            merge,
            line: line_no,
        })
    }

    pub(super) fn parse_split(&mut self, line_no: usize) -> Result<ActivityStmt> {
        self.pos += 1;
        let mut branches: Vec<Vec<ActivityStmt>> = Vec::new();
        let merge = true;
        loop {
            let (br, term) = self.parse_stmts(Terminator::EndSplit)?;
            branches.push(br);
            match term.kind {
                Terminator::SplitAgain => continue,
                Terminator::EndSplit => break,
                _ => break,
            }
        }
        Ok(ActivityStmt::Split {
            branches,
            merge,
            line: line_no,
        })
    }

    pub(super) fn parse_switch(&mut self, line_no: usize) -> Result<ActivityStmt> {
        let raw = self.lines[self.pos].text.trim().to_string();
        self.pos += 1;
        let cond = parse_paren_arg(&raw, "switch").unwrap_or_default();

        let mut cases: Vec<SwitchCase> = Vec::new();
        // PlantUML silently allows statements between `switch (…)` and
        // the first `case (…)` — eat them under a synthetic empty case.
        let (intro, mut term) = self.parse_stmts(Terminator::Case)?;
        if !intro.is_empty() {
            cases.push(SwitchCase {
                value: String::new(),
                branch: intro,
                line: line_no,
            });
        }
        while term.kind == Terminator::Case {
            let value = parse_paren_arg(&term.raw, "case").unwrap_or_default();
            let case_line = term.line;
            let (branch, next) = self.parse_stmts(Terminator::Case)?;
            cases.push(SwitchCase {
                value,
                branch,
                line: case_line,
            });
            term = next;
        }

        Ok(ActivityStmt::Switch {
            cond,
            cases,
            line: line_no,
        })
    }

    pub(super) fn parse_partition(
        &mut self,
        kind: PartitionKind,
        raw: &str,
        line_no: usize,
    ) -> Result<ActivityStmt> {
        self.pos += 1;
        let (label, color) = parse_partition_head(raw, kind);
        let (body, _term) = self.parse_stmts(Terminator::BraceClose)?;
        Ok(ActivityStmt::Partition {
            kind,
            label,
            color,
            body,
            line: line_no,
        })
    }
}
