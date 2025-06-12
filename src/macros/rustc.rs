use proc_macro::TokenStream;
use quote::quote;

use crate::utils::get_crate_name;

pub(super) fn flags_capture(args: TokenStream) -> TokenStream {
	let Some(crate_name) = get_crate_name() else {
		return args;
	};

	let flag = std::env::args().collect::<Vec<_>>();
	let flag_len = flag.len();
	let ret = quote! {
		pub static RUSTC_FLAGS: [&str; #flag_len] = [#( #flag ),*];

		#[tuwunel_core::ctor]
		fn _set_rustc_flags() {
			tuwunel_core::info::rustc::FLAGS.lock().expect("locked").insert(#crate_name, &RUSTC_FLAGS);
		}

		// static strings have to be yanked on module unload
		#[tuwunel_core::dtor]
		fn _unset_rustc_flags() {
			tuwunel_core::info::rustc::FLAGS.lock().expect("locked").remove(#crate_name);
		}
	};

	ret.into()
}
