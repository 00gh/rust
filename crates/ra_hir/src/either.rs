#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Either<A, B> {
    A(A),
    B(B),
}

impl<A, B> Either<A, B> {
    pub fn either<R, F1, F2>(self, f1: F1, f2: F2) -> R
    where
        F1: FnOnce(A) -> R,
        F2: FnOnce(B) -> R,
    {
        match self {
            Either::A(a) => f1(a),
            Either::B(b) => f2(b),
        }
    }
    pub fn map<U, V, F1, F2>(self, f1: F1, f2: F2) -> Either<U, V>
    where
        F1: FnOnce(A) -> U,
        F2: FnOnce(B) -> V,
    {
        match self {
            Either::A(a) => Either::A(f1(a)),
            Either::B(b) => Either::B(f2(b)),
        }
    }
}
