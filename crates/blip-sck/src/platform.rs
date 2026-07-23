use std::sync::Once;

use objc2_core_graphics::CGMainDisplayID;

pub(crate) fn initialize_core_graphics() {
    static INITIALIZE: Once = Once::new();
    INITIALIZE.call_once(|| {
        let _main_display_id = CGMainDisplayID();
    });
}
