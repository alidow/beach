#[cfg(feature = "alacritty-backend")]
mod alacritty_debug_test;
#[cfg(feature = "alacritty-backend")]
mod alacritty_mode_test;
mod backend_test;
mod background_color_test;
mod basic_test;
#[cfg(feature = "alacritty-backend")]
mod clear_screen_test;
#[cfg(feature = "alacritty-backend")]
mod cr_pending_wrap_test;
mod debug_test;
mod debug_view_test;
#[cfg(feature = "alacritty-backend")]
mod newline_behavior_test;
#[cfg(feature = "alacritty-backend")]
mod pending_wrap_test;
#[cfg(feature = "alacritty-backend")]
mod prompt_eol_multi_test;
#[cfg(feature = "alacritty-backend")]
mod prompt_eol_test;
#[cfg(feature = "alacritty-backend")]
mod resize_test;
mod scrollback_test;
mod test_utils;
mod utf8_test;
#[cfg(feature = "alacritty-backend")]
mod wrap_behavior_test;
