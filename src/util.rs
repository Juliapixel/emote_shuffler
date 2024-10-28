use rand::Rng;

pub fn shuffle_slice<T>(vec: &mut [T]) {
    let mut rng = rand::thread_rng();
    for i in (0..vec.len()).rev() {
        let idx = rng.gen_range(0..=i);
        vec.swap(idx, i);
    }
}
