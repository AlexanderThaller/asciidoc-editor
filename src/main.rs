mod app;
mod highlight;
mod render;
mod storage;
mod sync;

fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(app::App);
}
