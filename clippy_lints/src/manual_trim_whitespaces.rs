use clippy_utils::diagnostics::span_lint_and_sugg;
use clippy_utils::source::snippet;
use clippy_utils::{path_to_local_with_projections, sym};
use rustc_hir::{ClosureKind, Expr, ExprKind, PatKind, PathSegment};
use rustc_lint::{Applicability, LateContext, LateLintPass, declare_lint_pass};

declare_clippy_lint! {
    /// ### What it does
    /// It lints manual implementation of trim-like methods, which use
    /// `char::is_ascii_whitespace` / `char::is_whitespace` check in closures passed as a parameter
    /// ### Why is this bad?
    /// It makes code harder to read, and implements functionality, which is already present in stdlib.
    /// Also it allows
    /// ### Example
    /// ```no_run,rust
    /// let _ = "hello".trim_end_matches(|c: char| c.is_ascii_whitespace());
    /// let _ = "bye".trim_matches(|c: char| c.is_whitespace());
    /// ```
    /// Use instead:
    /// ```no_run,rust
    /// let _ = "hello".trim_ascii_end();
    /// let _ = "bye".trim();
    /// ```
    #[clippy::version = "1.102.0"]
    pub MANUAL_TRIM_WHITESPACES,
    complexity,
    "manual implementation of trim methods"
}

declare_lint_pass!(ManualTrimWhitespaces => [MANUAL_TRIM_WHITESPACES]);

impl<'tcx> LateLintPass<'tcx> for ManualTrimWhitespaces {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'_>) {
        if let ExprKind::MethodCall(seg, recv, [arg], _) = expr.kind
            && cx.typeck_results().expr_ty_adjusted(recv).peel_refs().is_str()
            && let Some(match_type) = extract_trim_method(seg)
            && let Some(pred_type) = extract_predicate_from_closure_expr(cx, arg)
        {
            let ascii_part = match pred_type {
                PredicateType::Plain => "",
                PredicateType::Ascii => "_ascii",
            };
            let position_part = match match_type {
                TrimPosition::Start => "_start",
                TrimPosition::End => "_end",
                TrimPosition::Both => "",
            };
            let reciever = snippet(cx, recv.span, "..");
            let method_name = format!("trim{ascii_part}{position_part}");
            let sugg = format!("{reciever}.{method_name}()");
            span_lint_and_sugg(
                cx,
                MANUAL_TRIM_WHITESPACES,
                expr.span,
                format!("manual implementation of `{method_name}` method"),
                "use",
                sugg,
                Applicability::MachineApplicable,
            );
        }
    }
}

enum PredicateType {
    Plain,
    Ascii,
}

fn extract_predicate_from_closure_expr(cx: &LateContext<'_>, expr: &'_ Expr<'_>) -> Option<PredicateType> {
    if let ExprKind::Closure(closure) = expr.kind
        && closure.kind == ClosureKind::Closure
        && let body = cx.tcx.hir_body(closure.body)
        && let [param] = body.params
        && let PatKind::Binding(_, param_hir_id, _, _) = param.pat.kind
        && let ExprKind::MethodCall(method_seg, closure_reciever, [], _) = body.value.kind
        && let Some(expr_reciever_hir_id) = path_to_local_with_projections(closure_reciever)
        && param_hir_id == expr_reciever_hir_id
    {
        return match method_seg.ident.name {
            sym::is_ascii_whitespace => Some(PredicateType::Ascii),
            sym::is_whitespace => Some(PredicateType::Plain),
            _ => None,
        };
    }
    None
}

enum TrimPosition {
    Start,
    End,
    Both,
}

fn extract_trim_method(seg: &PathSegment<'_>) -> Option<TrimPosition> {
    match seg.ident.name {
        sym::trim_matches => Some(TrimPosition::Both),
        sym::trim_start_matches => Some(TrimPosition::Start),
        sym::trim_end_matches => Some(TrimPosition::End),
        _ => None,
    }
}
