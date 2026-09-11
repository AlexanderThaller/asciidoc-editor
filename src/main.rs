mod app;
mod highlight;
mod inline;
mod list;
mod render;
mod source;
mod storage;
mod sync;
mod table;
mod wysiwyg;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
