use std::net::SocketAddr;

use super::covers;

fn addr(addr: &str) -> SocketAddr { addr.parse().expect("test address parses") }

#[test]
fn covers_the_same_address() {
	assert!(covers(&addr("127.0.0.1:8448"), &addr("127.0.0.1:8448")));
	assert!(covers(&addr("[::1]:8448"), &addr("[::1]:8448")));
}

#[test]
fn covers_its_family_from_the_wildcard() {
	assert!(covers(&addr("0.0.0.0:8448"), &addr("127.0.0.1:8448")));
	assert!(covers(&addr("[::]:8448"), &addr("[::1]:8448")));
}

#[test]
fn leaves_the_other_family_to_bind() {
	assert!(!covers(&addr("0.0.0.0:8448"), &addr("[::1]:8448")));
	assert!(!covers(&addr("[::]:8448"), &addr("127.0.0.1:8448")));
}

#[test]
fn leaves_another_port_to_bind() {
	assert!(!covers(&addr("0.0.0.0:8448"), &addr("127.0.0.1:8008")));
	assert!(!covers(&addr("127.0.0.1:8448"), &addr("127.0.0.1:8008")));
}

#[test]
fn a_specific_address_covers_no_wildcard() {
	assert!(!covers(&addr("127.0.0.1:8448"), &addr("0.0.0.0:8448")));
}

#[cfg(unix)]
#[test]
fn same_path_matches_one_socket() {
	use std::path::Path;

	use super::same_path;

	assert!(same_path(
		Path::new("/run/tuwunel/tuwunel.sock"),
		Path::new("/run/tuwunel/tuwunel.sock")
	));
	assert!(!same_path(Path::new("/run/tuwunel/tuwunel.sock"), Path::new("/run/other.sock")));
}
