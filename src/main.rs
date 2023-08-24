/// Simulation of Mother Brain's neck, to find and analyze strats for standup manip.
/// By NobodyNada -- with credit to ShinyZeni, ProfessorSchool, sniq, PJBoy, and cpadolf for helping
/// out with this in one way or another :)
///
/// I recommend running with optimizations, as it does a pretty big brute force. It runs in about 15
/// seconds on my machine, but if your computer is slower or you want to iterate faster, it should
/// be pretty easy to parallelize with rayon: https://crates.io/crates/rayon
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{BufRead, BufReader, Write},
};

use image::{ImageBuffer, ImageOutputFormat, Rgb};
use smallvec::SmallVec;

#[allow(clippy::tabs_in_doc_comments)]
/// This program requires data logged from vanilla SM, while Mother Brain's neck is bobbing back
/// and forth after she is grabbed by the baby.
/// The input was generated in lsnes using the following Lua script:
/// ```lua
/// function on_frame()
///     print(
///         memory.readword(0x7E7816),
///         memory.readword(0x7E8068),
///         memory.readword(0x7E8040),
///         memory.readword(0x7E8042),
///         memory.readword(0x7E805E)
///     )
/// end
/// ```
/// I've provided my trace in the file 'trace.txt'. If you want to generate it yourself, make sure
/// to filter it to only include the relevant frames -- it should start and end like this:
/// ```
/// 196	1792	36864	38912	64
/// 196	1792	35072	37120	61
/// 196	1280	33792	35840	60
/// 196	1280	32512	34560	60
/// ...
/// 196	64	12928	14976	108
/// 196	64	12864	14912	108
/// 196	64	12800	14848	108
/// 196	64	12736	14784	110
/// 196	64	12672	14720	110
/// ```
#[derive(Debug)]
struct Frame {
    body_y: u16,
    angle_delta: u16,
    expected_lower_angle: u16,
    expected_upper_angle: u16,
    expected_brain_y: u16,
}

/// Our simulation of Mother Brain's neck state.
#[derive(Hash, PartialEq, Eq, Clone, Copy, Debug)]
struct MotherBrain {
    /// The angle between her torso and bottom 2 neck joints.
    /// $7E:8040 in vanilla SM.
    lower_angle: u16,

    /// The angle between her torso and top 2 neck joints.
    /// $7E:8042
    upper_angle: u16,

    /// Whether her lower neck joints are bobbing up.
    /// If true, $7E:8064 is 4. If false, it's 2.
    lower_moving_up: bool,

    /// Whether her upper neck joints are bobbing up.
    /// If true, $7E:8066 is 4. If false, it's 2.
    upper_moving_up: bool,
}

impl MotherBrain {
    /// Simulates Mother Brain's neck for one frame ($A9:91B8).
    /// - body_y: The Y position of MB's torso.
    /// - delta: How fast to move the neck.
    /// - samus_jumped: Whether Samus is above MB's head.
    fn run_frame(&mut self, body_y: u16, delta: u16, samus_jumped: bool) {
        let brain_y = self.brain_y(body_y);

        if self.lower_moving_up {
            if brain_y < 0x3C {
                self.lower_moving_up = false;
            } else {
                self.lower_angle += delta;
                if self.lower_angle >= 0x9000 {
                    self.lower_moving_up = false;
                    self.lower_angle = 0x9000;
                }
            }
        } else {
            self.lower_angle -= delta;
            if self.lower_angle < 0x2800 {
                self.lower_angle = 0x2800;
                self.lower_moving_up = true;
            }
        }

        if self.upper_moving_up {
            let max = self.lower_angle + 0x800;
            self.upper_angle += delta;
            if self.upper_angle >= max {
                self.upper_moving_up = false;
                self.upper_angle = max;
            }
        } else if samus_jumped {
            self.upper_moving_up = true;
            self.lower_moving_up = true;
        } else {
            self.upper_angle -= delta;
            if self.upper_angle < 0x2000 {
                self.upper_moving_up = true;
                self.upper_angle = 0x2000;
            }
        }
    }

    /// Computes the position of MB's head, given her body position and her current neck state.
    /// $A9:91DA
    fn brain_y(&self, body_y: u16) -> u16 {
        let base_y = body_y - 0x60;
        let distance_2 = 20.;
        let lower_angle = (self.lower_angle / 0x100) as f64 * std::f64::consts::PI / 128.;
        let seg2_y = (base_y as i16) + (distance_2 * lower_angle.cos()).floor() as i16;

        let distance_4 = 20.;
        let upper_angle = (self.upper_angle / 0x100) as f64 * std::f64::consts::PI / 128.;
        ((seg2_y) + (distance_4 * upper_angle.cos()).floor() as i16 - 0x15) as u16
    }
}

fn main() {
    // Read the input trace.
    let trace = BufReader::new(File::open("trace.txt").expect(
        "this program requires a trace from vanilla SM, \
         make sure 'trace.txt' is present in the current directory.",
    ))
    .lines()
    .map(|line| {
        let line = line.unwrap();
        // Split on tabs & parse
        let mut cols = line
            .split('\t')
            .map(|col| col.parse::<u16>().expect("could not parse column"));
        let result = Frame {
            body_y: cols.next().unwrap(),
            angle_delta: cols.next().unwrap(),
            expected_lower_angle: cols.next().unwrap(),
            expected_upper_angle: cols.next().unwrap(),
            expected_brain_y: cols.next().unwrap(),
        };
        assert_eq!(cols.next(), None, "extra columns");
        result
    })
    .collect::<Vec<Frame>>();

    // An array of (parent_index, jumping) tuples, where jumping is either true, false, or None
    type Parents = SmallVec<[(u32, Option<bool>); 4]>;
    let mut all_states = vec![vec![(
        MotherBrain {
            lower_angle: 0x9000,
            upper_angle: 0x9800,
            lower_moving_up: false,
            upper_moving_up: false,
        },
        Parents::default(),
    )]];

    let mut new_states = HashMap::<MotherBrain, Parents>::new();
    for (i, frame) in trace.iter().enumerate() {
        eprintln!("frame {i}, {} states", all_states[i].len());
        let delta = trace
            .get(i + 1)
            .unwrap_or(trace.last().unwrap())
            .angle_delta;

        for (j, prev) in all_states[i].iter().enumerate() {
            let mut jumping = prev.0;
            jumping.run_frame(frame.body_y, delta, true);

            let mut not_jumping = prev.0;
            not_jumping.run_frame(frame.body_y, delta, false);

            if jumping == not_jumping {
                // It doesn't matter whether we jump or not.
                new_states
                    .entry(jumping)
                    .or_insert(SmallVec::new())
                    .push((j.try_into().unwrap(), None));
            } else {
                new_states
                    .entry(jumping)
                    .or_insert(SmallVec::new())
                    .push((j.try_into().unwrap(), Some(true)));

                new_states
                    .entry(not_jumping)
                    .or_insert(SmallVec::new())
                    .push((j.try_into().unwrap(), Some(false)));
            }
        }

        all_states.push(new_states.drain().collect());
    }
    //end_states.sort_by_key(|s| s.0.lower_angle);
    //println!("{:#?}", end_states);

    struct Path {
        states: SmallVec<[u32; 8]>,
        inputs: Vec<Option<bool>>,
    }
    impl Path {
        fn difficulty(&self) -> usize {
            let mut result = 0;
            let mut prev = false;
            let mut prev_time = 1000000;
            for &input in &self.inputs {
                if let Some(i) = input {
                    if prev == i {
                        if i {
                            result += 1;
                        }
                    } else if prev && !i {
                        // switches are hard
                        result += 10000 / (prev_time + 1usize).pow(2u32);
                    }

                    prev = i;
                    prev_time = 0;
                } else {
                    prev_time += 1;
                }
            }
            result
        }
    }

    let mut paths = vec![Path {
        states: all_states
            .last()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(i, s)| s.0.lower_angle >= 0x8000)
            .map(|(i, _)| i as u32)
            .collect(),
        inputs: vec![],
    }];

    for (i, states) in all_states.iter().enumerate().skip(1).rev() {
        eprintln!("frame {i}, {} paths", paths.len());
        let prev_states = &all_states[i - 1];

        paths = paths
            .drain(..)
            .flat_map(|path| {
                // States which lead into a state within this path regardless of Samus action.
                let mut x = HashSet::<u32>::new();
                // States which lead into a state within this path if Samus is above MB.
                let mut y = HashSet::<u32>::new();
                // States which lead into a state within this path if Samus is above MB.
                let mut n = HashSet::<u32>::new();

                for &(parent, input) in path
                    .states
                    .iter()
                    .flat_map(|s| states[*s as usize].1.iter())
                {
                    match input {
                        _ if x.contains(&parent) => {}
                        None => {
                            n.remove(&parent);
                            y.remove(&parent);
                            x.insert(parent);
                        }
                        Some(true) => {
                            if n.remove(&parent) {
                                x.insert(parent);
                            } else {
                                y.insert(parent);
                            }
                        }
                        Some(false) => {
                            if y.remove(&parent) {
                                x.insert(parent);
                            } else {
                                n.insert(parent);
                            }
                        }
                    }
                }

                [
                    if !x.is_empty() {
                        let mut inputs = path.inputs.clone();
                        inputs.push(None);
                        Some(Path {
                            states: x.drain().collect(),
                            inputs,
                        })
                    } else {
                        None
                    },
                    if !y.is_empty() {
                        let mut inputs = path.inputs.clone();
                        inputs.push(Some(true));
                        Some(Path {
                            states: y.drain().collect(),
                            inputs,
                        })
                    } else {
                        None
                    },
                    if !n.is_empty() {
                        let mut inputs = path.inputs;
                        inputs.push(Some(false));
                        Some(Path {
                            states: n.drain().collect(),
                            inputs,
                        })
                    } else {
                        None
                    },
                ]
                .into_iter()
                .flatten()
            })
            .collect();

        // to keep things under control, only keep the paths with the fewest input requirements
        let max = 100000;
        if paths.len() > max {
            paths.sort_by_cached_key(|path| path.difficulty());
            paths.drain(max..);
        }
    }

    let max = 1000;
    if paths.len() > max {
        paths.sort_by_cached_key(|path| path.difficulty());
        paths.drain(max..);
    }

    let width = trace.len();
    let height = paths.len();

    let mut image = ImageBuffer::<Rgb<u8>, _>::new(width as u32, height as u32);

    for (y, path) in paths.iter().enumerate() {
        for (x, input) in path.inputs.iter().enumerate() {
            image.put_pixel(
                (width - x - 1) as u32,
                y as u32,
                match input {
                    None => Rgb([255, 255, 255]),
                    Some(true) => Rgb([0, 255, 0]),
                    Some(false) => Rgb([255, 0, 0]),
                },
            )
        }
    }

    let mut buf = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut buf), ImageOutputFormat::Png)
        .expect("failed to encode image");
    std::io::stdout()
        .write_all(&buf)
        .expect("failed to write image");
}

/// Runs a simulation of a complete cutscene.
/// - trace: The input trace.
/// - jump_frames: A function to determine, given a frame number,
///   whether Samus is above MB during that frame.
fn simulate(trace: &[Frame], mut jump_frames: impl FnMut(usize) -> bool) -> u16 {
    let mut mb = MotherBrain {
        lower_angle: 0x9000,
        upper_angle: 0x9800,
        lower_moving_up: false,
        upper_moving_up: false,
    };

    let mut jumped = false;
    for (i, frame) in trace.iter().enumerate() {
        if !jumped {
            // Sanity check: if our inputs match the Lua dump, make sure our outputs do too
            assert_eq!(
                mb.lower_angle, frame.expected_lower_angle,
                "frame {i}, lower"
            );
            assert_eq!(
                mb.upper_angle, frame.expected_upper_angle,
                "frame {i}, upper"
            );
            assert_eq!(
                mb.brain_y(frame.body_y),
                frame.expected_brain_y - 0x15,
                "frame {i}, brain_y"
            );
        }

        let delta = trace
            .get(i + 1)
            .unwrap_or(trace.last().unwrap())
            .angle_delta;
        let should_jump = jump_frames(i);
        jumped |= should_jump;
        mb.run_frame(frame.body_y, delta, should_jump);
    }

    mb.lower_angle
}
