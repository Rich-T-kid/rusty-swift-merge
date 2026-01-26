#[derive(Debug)]
pub enum LsmTreeError {
    Unimplemented(),
    ErrorFlushing(),
}
