//! SPDX-License-Identifier: MIT OR Apache-2.0

use std::{convert::From, ffi::OsString, collections::HashMap};


#[derive(Debug)]
pub struct CapturedEnv {
  // pub captured: Vec<(OsString, OsString)>,
  pub map: HashMap<OsString, OsString>,
}

impl Default for CapturedEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl CapturedEnv {
  pub fn new() -> Self {
    Self {
      // captured: std::env::vars_os().collect(),
      map: std::env::vars_os().collect::<HashMap<OsString, OsString>>(),
    }
  }
}

impl From<Vec<(OsString, OsString)>> for CapturedEnv {
  fn from(other: Vec<(OsString, OsString)>) -> Self {
    let mut e = Self::new();
    e.map.extend(other);
    e
  }
}

#[test]
fn extend_env() {
  let e = CapturedEnv::from(vec![(OsString::from("myvar"),OsString::from("myvalue"))]);
  assert_eq!(e.map.get(OsString::from("myvar").as_os_str()), Some(&OsString::from("myvalue")));
}