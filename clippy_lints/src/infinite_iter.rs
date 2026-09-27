use clippy_utils::consts::{ConstEvalCtxt, Constant};
use clippy_utils::diagnostics::span_lint;
use clippy_utils::res::MaybeDef as _;
use clippy_utils::ty::implements_trait;
use clippy_utils::visitors::find_all_ret_expressions;
use clippy_utils::{as_some_expr, higher, is_none_expr, sym};
use rustc_hir::{Body, BorrowKind, Closure, Expr, ExprKind};
use rustc_lint::{LateContext, LateLintPass, declare_lint_pass};
use rustc_span::Symbol;

declare_clippy_lint! {
    /// ### What it does
    /// Checks for iteration that is guaranteed to be infinite.
    ///
    /// ### Why is this bad?
    /// While there may be places where this is acceptable
    /// (e.g., in event streams), in most cases this is simply an error.
    ///
    /// ### Example
    /// ```no_run
    /// use std::iter;
    ///
    /// iter::repeat(1_u8).collect::<Vec<_>>();
    /// ```
    #[clippy::version = "pre 1.29.0"]
    pub INFINITE_ITER,
    correctness,
    "infinite iteration"
}

declare_clippy_lint! {
    /// ### What it does
    /// Checks for iteration that may be infinite.
    ///
    /// ### Why is this bad?
    /// While there may be places where this is acceptable
    /// (e.g., in event streams), in most cases this is simply an error.
    ///
    /// ### Known problems
    /// The code may have a condition to stop iteration, but
    /// this lint is not clever enough to analyze it.
    ///
    /// ### Example
    /// ```no_run
    /// let infinite_iter = 0..;
    /// [0..].iter().zip(infinite_iter.take_while(|x| *x > 5));
    /// ```
    #[clippy::version = "pre 1.29.0"]
    pub MAYBE_INFINITE_ITER,
    pedantic,
    "possible infinite iteration"
}

declare_lint_pass!(InfiniteIter => [INFINITE_ITER, MAYBE_INFINITE_ITER]);

impl<'tcx> LateLintPass<'tcx> for InfiniteIter {
    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx Expr<'_>) {
        let (lint, msg) = match complete_infinite_iter(cx, expr) {
            Infinite => (INFINITE_ITER, "infinite iteration detected"),
            MaybeInfinite => (MAYBE_INFINITE_ITER, "possible infinite iteration detected"),
            Finite => {
                return;
            },
        };
        span_lint(cx, lint, expr.span, msg);
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Finiteness {
    Infinite,
    MaybeInfinite,
    Finite,
}

use self::Finiteness::{Finite, Infinite, MaybeInfinite};

impl Finiteness {
    #[must_use]
    fn and(self, b: Self) -> Self {
        match (self, b) {
            (Finite, _) | (_, Finite) => Finite,
            (MaybeInfinite, _) | (_, MaybeInfinite) => MaybeInfinite,
            _ => Infinite,
        }
    }

    #[must_use]
    fn or(self, b: Self) -> Self {
        match (self, b) {
            (Infinite, _) | (_, Infinite) => Infinite,
            (MaybeInfinite, _) | (_, MaybeInfinite) => MaybeInfinite,
            _ => Finite,
        }
    }
}

impl From<bool> for Finiteness {
    fn from(b: bool) -> Self {
        if b { Infinite } else { Finite }
    }
}

/// This tells us what to look for to know if the iterator returned by
/// this method is infinite
#[derive(Copy, Clone)]
enum Heuristic {
    /// infinite no matter what
    Always,
    /// infinite if the first argument is
    First,
    /// infinite if any of the supplied arguments is
    Any,
    /// infinite if all of the supplied arguments are
    All,
}

use self::Heuristic::{All, Always, Any, First};

/// An enum that represents a bound for finitness
/// or other form of requierements, so it would be safe to say
/// that a method or a function can be infinite or not
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Cap {
    Constant(Finiteness),
    /// usize in ClosureTrue and ClosureSome is the index of the argument
    /// that is requiered to be a closure always returning true/Some(_) respectively
    ClosureTrue(usize),
    ClosureSome(usize),
}

/// a slice of (method name, number of args, heuristic, bounds) tuples
/// that will be used to determine whether the method in question
/// returns an infinite or possibly infinite iterator. The finiteness
/// is an upper bound, e.g., some methods can return a possibly
/// infinite iterator at worst, e.g., `take_while`.
const HEURISTICS: [(Symbol, usize, Heuristic, Cap); 20] = [
    (sym::zip, 1, All, Cap::Constant(Infinite)),
    (sym::chain, 1, Any, Cap::Constant(Infinite)),
    (sym::cycle, 0, Always, Cap::Constant(Infinite)),
    (sym::map, 1, First, Cap::Constant(Infinite)),
    (sym::by_ref, 0, First, Cap::Constant(Infinite)),
    (sym::cloned, 0, First, Cap::Constant(Infinite)),
    (sym::rev, 0, First, Cap::Constant(Infinite)),
    (sym::inspect, 0, First, Cap::Constant(Infinite)),
    (sym::enumerate, 0, First, Cap::Constant(Infinite)),
    (sym::peekable, 1, First, Cap::Constant(Infinite)),
    (sym::fuse, 0, First, Cap::Constant(Infinite)),
    (sym::skip, 1, First, Cap::Constant(Infinite)),
    (sym::skip_while, 0, First, Cap::Constant(Infinite)),
    (sym::filter, 1, First, Cap::Constant(Infinite)),
    (sym::filter_map, 1, First, Cap::Constant(Infinite)),
    (sym::flat_map, 1, First, Cap::Constant(Infinite)),
    (sym::unzip, 0, First, Cap::Constant(Infinite)),
    (sym::take_while, 1, First, Cap::ClosureTrue(0)),
    (sym::scan, 2, First, Cap::ClosureSome(1)),
    (sym::map_while, 1, First, Cap::ClosureSome(0)),
];

fn closure_body_always_returns_true(cx: &LateContext<'_>, body: &Body) -> bool {
    find_all_ret_expressions(cx, body.value, |e| {
        matches!(ConstEvalCtxt::new(cx).eval(e), Some(Constant::Bool(true)))
    })
}

fn closure_body_always_returns_some(cx: &LateContext<'_>, body: &Body) -> bool {
    find_all_ret_expressions(cx, body.value, |ret_expr| as_some_expr(cx, ret_expr).is_some())
}

fn is_infinite(cx: &LateContext<'_>, expr: &Expr<'_>) -> Finiteness {
    match expr.kind {
        ExprKind::MethodCall(method, receiver, args, _) => {
            for &(name, len, heuristic, cap) in &HEURISTICS {
                if method.ident.name == name && args.len() == len {
                    let base = match heuristic {
                        Always => Infinite,
                        First => is_infinite(cx, receiver),
                        Any => is_infinite(cx, receiver).or(is_infinite(cx, &args[0])),
                        All => is_infinite(cx, receiver).and(is_infinite(cx, &args[0])),
                    };
                    let term = match cap {
                        Cap::Constant(c) => c,
                        Cap::ClosureTrue(i) => {
                            if expr_is_closure_always_returns_true(cx, &args[i]) {
                                Infinite
                            } else {
                                MaybeInfinite
                            }
                        },
                        Cap::ClosureSome(i) => {
                            if expr_is_closure_always_returns_some(cx, &args[i]) {
                                Infinite
                            } else {
                                MaybeInfinite
                            }
                        },
                    };
                    return base.and(term);
                }
            }
            if method.ident.name == sym::flat_map
                && let [single] = args
                && let ExprKind::Closure(&Closure { body, .. }) = single.kind
            {
                let body = cx.tcx.hir_body(body);
                is_infinite(cx, body.value)
            } else {
                Finite
            }
        },
        ExprKind::Block(block, _) => block.expr.as_ref().map_or(Finite, |e| is_infinite(cx, e)),
        ExprKind::AddrOf(BorrowKind::Ref, _, e) => is_infinite(cx, e),
        ExprKind::Call(path, args) => {
            if let ExprKind::Path(ref qpath) = path.kind {
                if let Some(def_id) = cx.qpath_res(qpath, path.hir_id).opt_def_id() {
                    if cx.tcx.is_diagnostic_item(sym::iter_repeat_with, def_id)
                        || cx.tcx.is_diagnostic_item(sym::iter_repeat, def_id)
                    {
                        dbg!("AMOGUS");
                        Infinite
                    } else if cx.tcx.is_diagnostic_item(sym::iter_from_fn, def_id) {
                        if let Some(e) = args.first()
                            && expr_is_closure_always_returns_some(cx, e)
                        {
                            Infinite
                        } else {
                            MaybeInfinite
                        }
                    } else if cx.tcx.is_diagnostic_item(sym::iter_successors, def_id) {
                        if let [seed, succ_func] = args {
                            if is_none_expr(cx, seed) {
                                Finite
                            } else if expr_is_closure_always_returns_some(cx, succ_func) {
                                Infinite
                            } else {
                                MaybeInfinite
                            }
                        } else {
                            Finite
                        }
                    } else {
                        Finite
                    }
                } else {
                    Finite
                }
            } else {
                Finite
            }
        },
        ExprKind::Struct(..) => higher::Range::hir(cx, expr).is_some_and(|r| r.end.is_none()).into(),
        _ => Finite,
    }
}

fn expr_is_closure_always_returns_true(cx: &LateContext<'_>, expr: &Expr<'_>) -> bool {
    if let ExprKind::Closure(Closure { body, .. }) = expr.kind {
        closure_body_always_returns_true(cx, cx.tcx.hir_body(*body))
    } else {
        false
    }
}

fn expr_is_closure_always_returns_some(cx: &LateContext<'_>, expr: &Expr<'_>) -> bool {
    if let ExprKind::Closure(Closure { body, .. }) = expr.kind {
        closure_body_always_returns_some(cx, cx.tcx.hir_body(*body))
    } else {
        false
    }
}

/// the names and argument lengths of methods that *may* exhaust their
/// iterators
const POSSIBLY_COMPLETING_METHODS: [(Symbol, usize); 6] = [
    (sym::find, 1),
    (sym::rfind, 1),
    (sym::position, 1),
    (sym::rposition, 1),
    (sym::any, 1),
    (sym::all, 1),
];

/// the names and argument lengths of methods that *always* exhaust
/// their iterators
const COMPLETING_METHODS: [(Symbol, usize); 12] = [
    (sym::count, 0),
    (sym::fold, 2),
    (sym::for_each, 1),
    (sym::partition, 1),
    (sym::max, 0),
    (sym::max_by, 1),
    (sym::max_by_key, 1),
    (sym::min, 0),
    (sym::min_by, 1),
    (sym::min_by_key, 1),
    (sym::sum, 0),
    (sym::product, 0),
];

fn complete_infinite_iter(cx: &LateContext<'_>, expr: &Expr<'_>) -> Finiteness {
    match expr.kind {
        ExprKind::MethodCall(method, receiver, args, _) => {
            let method_str = method.ident.name;
            for &(name, len) in &COMPLETING_METHODS {
                if method_str == name && args.len() == len {
                    return is_infinite(cx, receiver);
                }
            }
            for &(name, len) in &POSSIBLY_COMPLETING_METHODS {
                if method_str == name && args.len() == len {
                    return MaybeInfinite.and(is_infinite(cx, receiver));
                }
            }
            if method.ident.name == sym::last && args.is_empty() {
                let not_double_ended = cx
                    .tcx
                    .get_diagnostic_item(sym::DoubleEndedIterator)
                    .is_some_and(|id| !implements_trait(cx, cx.typeck_results().expr_ty(receiver), id, &[]));
                if not_double_ended {
                    return is_infinite(cx, receiver);
                }
            } else if method.ident.name == sym::collect {
                let ty = cx.typeck_results().expr_ty(expr);
                if matches!(
                    ty.opt_diag_name(cx),
                    Some(
                        sym::BinaryHeap
                            | sym::BTreeMap
                            | sym::BTreeSet
                            | sym::HashMap
                            | sym::HashSet
                            | sym::LinkedList
                            | sym::Vec
                            | sym::VecDeque,
                    )
                ) {
                    return is_infinite(cx, receiver);
                }
            }
        },
        ExprKind::Binary(op, l, r) if op.node.is_comparison() => {
            return is_infinite(cx, l).and(is_infinite(cx, r)).and(MaybeInfinite);
        }, // TODO: ExprKind::Loop + Match
        _ => (),
    }
    Finite
}
