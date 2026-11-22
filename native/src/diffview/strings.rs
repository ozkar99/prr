//! English UI strings for the vendored diff view (gpui-component's locales/ui.yml, `Diff.*`).
//! Replaces its rust_i18n `t!` lookups; `%{name}` placeholders are filled by `t!`.
pub(crate) fn english(key: &str) -> &'static str {
    match key {
        "Diff.AcceptBoth" => "Accept Both",
        "Diff.AcceptCurrent" => "Accept Current",
        "Diff.AcceptIncoming" => "Accept Incoming",
        "Diff.AcceptedBoth" => "Accepted both changes",
        "Diff.AcceptedCurrent" => "Accepted current change",
        "Diff.AcceptedIncoming" => "Accepted incoming change",
        "Diff.AddAnnotation" => "Add annotation to line %{line}",
        "Diff.AddedFile" => "Added file",
        "Diff.AddedLine" => "%{side} line %{line}, added",
        "Diff.BaseChange" => "Common ancestor",
        "Diff.BinaryChanges" => "Binary file changed",
        "Diff.CollapseFile" => "Collapse %{name}",
        "Diff.ConflictedFile" => "Conflicts",
        "Diff.CopiedFile" => "Copied",
        "Diff.CurrentChange" => "Current change",
        "Diff.DeletedFile" => "Deleted file",
        "Diff.EmptyFile" => "Empty file",
        "Diff.ExpandAbove" => "Show %{count} lines above",
        "Diff.ExpandBelow" => "Show %{count} lines below",
        "Diff.ExpandFile" => "Expand %{name}",
        "Diff.ExpandLines" => "Expand %{count} unchanged lines",
        "Diff.HiddenLines" => "%{count} lines not in patch",
        "Diff.IncomingChange" => "Incoming change",
        "Diff.Modified" => "Modified",
        "Diff.NoChanges" => "No changes",
        "Diff.NoNewline" => "No newline at end of file",
        "Diff.NoTextChanges" => "No text changes in this patch",
        "Diff.Original" => "Original",
        "Diff.RemovedLine" => "%{side} line %{line}, removed",
        "Diff.RenamedFile" => "Renamed",
        "Diff.SelectLine" => "Select %{side} line %{line}",
        "Diff.SourceLine" => "%{side} line %{line}",
        "Diff.UnchangedLines" => "%{count} unchanged lines",
        "Diff.UndoResolution" => "Undo",
        "Diff.Viewer" => "Diff for %{name}",
        "Diff.ViewerFiles" => "Diff for %{count} files",
        _ => "",
    }
}

/// `t!("Diff.Key")` or `t!("Diff.Key", name = value, ...)`, like rust_i18n's.
macro_rules! t {
    ($key:expr) => { std::borrow::Cow::<'static, str>::Borrowed($crate::diffview::strings::english($key)) };
    ($key:expr, $($name:ident = $value:expr),+ $(,)?) => {{
        let mut s = $crate::diffview::strings::english($key).to_string();
        $( s = s.replace(concat!("%{", stringify!($name), "}"), &$value.to_string()); )+
        std::borrow::Cow::<'static, str>::Owned(s)
    }};
}
pub(crate) use t;
