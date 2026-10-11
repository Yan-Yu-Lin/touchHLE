/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The default BSD `random()` generator (31 words, additive feedback).
//!
//! Games may depend on its exact sequence to decode bundled data files.
//! See Apple's random(3) manual for the state size and default seed.

pub(super) struct Random {
    words: [u32; 31],
    index: usize,
}

impl Default for Random {
    fn default() -> Self {
        Self::new(1)
    }
}

impl Random {
    pub(super) fn new(seed: u32) -> Self {
        let mut generator = Self {
            words: [0; 31],
            index: 0,
        };
        generator.words[0] = seed;
        for i in 1..31 {
            let previous = generator.words[i - 1] as i32 as i64;
            let previous = if previous == 0 { 123459876 } else { previous };
            generator.words[i] = (previous * 16807).rem_euclid(2147483647) as u32;
        }
        // Discard ten rounds to mix the initial state.
        for _ in 0..310 {
            generator.next();
        }
        generator
    }

    pub(super) fn next(&mut self) -> i32 {
        let front = (self.index + 3) % self.words.len();
        let sum = self.words[front].wrapping_add(self.words[self.index]);
        self.words[front] = sum;
        self.index = (self.index + 1) % self.words.len();
        (sum >> 1) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::Random;

    #[test]
    fn darwin_sequences() {
        // Reference outputs obtained by calling macOS libc, not its source.
        let cases = [
            (0, [577655601, 1248161417, 192959563, 143633695]),
            (1, [1804289383, 846930886, 1681692777, 1714636915]),
            (731127, [134421688, 624679831, 1175454998, 389078347]),
            (u32::MAX, [254925627, 1205188300, 366127624, 1401405153]),
        ];
        for (seed, expected) in cases {
            let mut generator = Random::new(seed);
            for value in expected {
                assert_eq!(generator.next(), value, "seed {seed}");
            }
        }
        assert_eq!(Random::default().next(), Random::new(1).next());
    }
}
