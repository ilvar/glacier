use proptest::collection::vec;
use proptest::prelude::{any, prop_assert_eq, proptest};

proptest! {
    #[test]
    fn reversing_twice_preserves_values(values in vec(any::<u8>(), 0..256)) {
        let mut transformed = values.clone();
        transformed.reverse();
        transformed.reverse();

        prop_assert_eq!(transformed, values);
    }
}
