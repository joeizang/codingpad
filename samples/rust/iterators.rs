use codingpad::dump;

let readings = [18, 22, 19, 24, 21];
let warm: Vec<_> = readings.iter().copied().filter(|&degrees| degrees >= 21).collect();
dump(&warm);

warm.iter().sum::<i32>() as f64 / warm.len() as f64
