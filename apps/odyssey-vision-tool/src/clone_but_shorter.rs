pub trait CloneButShorter: Clone {
    /// Use mainly for GUI code.
    fn c(&self) -> Self {
        self.clone()
    }
}

impl<T: Clone> CloneButShorter for T {}
