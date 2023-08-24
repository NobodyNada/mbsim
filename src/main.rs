/// Simulation of Mother Brain's neck, to find and analyze strats for standup manip.
/// By NobodyNada -- with credit to ShinyZeni, ProfessorSchool, sniq, PJBoy, and cpadolf for helping
/// out with this in one way or another :)
///
/// I recommend running with optimizations, as it does a pretty big brute force. It runs in about 15
/// seconds on my machine, but if your computer is slower or you want to iterate faster, it should
/// be pretty easy to parallelize with rayon: https://crates.io/crates/rayon
use std::{
    fs::File,
    io::{BufRead, BufReader},
};

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
    let frames = BufReader::new(File::open("trace.txt").expect(
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

    // For all possible time intervals (a, b), evaluate the following strategy:
    // - Start out on the ground
    // - Jump in the air at time a
    // - Come back down to the ground at time b
    let results = (0..frames.len())
        .flat_map(|a| (0..frames.len()).map(move |b| (a, b)))
        .map(|(a, b)| (a, b, simulate(&frames, |j| a < j && j < b)))
        // Show all results better than a threshold. This way, we can visually inspect the output
        // for "clusters" of especially good results -- for the purposes
        // of finding lenient, RTA-viable setups.
        .filter(|(_a, _b, x)| *x > 0x8000);
    for (a, b, angle) in results {
        println!("{a}-{b}:\t{angle:#04X}");
    }
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
