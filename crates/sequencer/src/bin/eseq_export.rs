fn main() -> Result<(), Box<dyn std::error::Error>> {
    sequencer::bounce::command::run(std::env::args().skip(1))
}
