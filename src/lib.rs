// SPDX-License-Identifier: Apache-2.0
// Copyright (C) 2024, Marco Elver <me@marcoelver.com>

pub mod evolve;
pub mod ga;
pub mod midi;
pub mod sequencer;
pub mod units;
pub mod vm;

pub type Result<T> = std::result::Result<T, String>;
