#![warn(clippy::manual_trim_whitespaces)]

fn main() {
    let some_str = " foo ";
    let _ = some_str.trim_end_matches(|kek: char| kek.is_ascii_whitespace());
    //~^ manual_trim_whitespaces

    let _ = some_str.trim_matches(|it: char| it.is_ascii_whitespace());
    //~^ manual_trim_whitespaces

    let r = ' ';
    let _ = some_str.trim_start_matches(|rog: char| rog.is_whitespace());
    //~^ manual_trim_whitespaces

    // works with literals
    let _ = "big_chungus".trim_start_matches(|it: char| it.is_ascii_whitespace());
    //~^ manual_trim_whitespaces

    // works within method chains
    //~v manual_trim_whitespaces
    let _ = "big_chungus"
        .repeat(2)
        .trim_matches(|it: char| it.is_ascii_whitespace());

    // works inside functions
    let _ = "boo".replace("bo", " big_chungus ".trim_matches(|it: char| it.is_ascii_whitespace()));
    //~^ manual_trim_whitespaces

    // works inside functions
    let _ = "boo".replace("bo", " big_chungus ".trim_matches(|it: char| it.is_ascii_whitespace()));
    //~^ manual_trim_whitespaces

    // no trigger on wrong method call
    let _ = "qwe".matches(|ch: char| ch.is_ascii_whitespace());

    // no trigger on wrong method call
    let _ = "".trim_matches(|ch: char| ch.is_ascii_control());

    // no trigger on different reciever call in closure
    let chr = 'p';
    let _ = "".trim_matches(|c: char| chr.is_ascii_whitespace());

    // no trigger on non-str reciever
    let _ = Dummy {}.trim_matches(|c: char| c.is_ascii_whitespace());
}

struct Dummy {}
impl Dummy {
    pub fn trim_matches<F>(&self, pat: F) -> &str
    where
        F: FnOnce(char) -> bool,
    {
        "dummy"
    }
}
