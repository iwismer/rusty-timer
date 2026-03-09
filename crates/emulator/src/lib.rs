pub mod control_handler;
pub mod faults;
pub mod read_gen;
pub mod scenario;
pub mod server;

/// LCG: x_{n+1} = (a * x_n + c) mod 2^64
///
/// Constants from Numerical Recipes. Used for deterministic chip selection
/// in both scenario event generation and download read simulation.
pub(crate) fn lcg_next(state: u64) -> u64 {
    state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
}
