//! Run one isolated audio experiment per process; JSON config in, JSON result out.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: audio_experiment CONFIG.json")?;
    let config = serde_json::from_slice(&std::fs::read(path)?)?;
    let result = sequencer::audio::experiment::run(config)?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
