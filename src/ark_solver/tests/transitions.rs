use ark_solver::simulation::Weighted;

#[test]
fn weighted_values_reject_invalid_probabilities() {
	for probability in [-0.01, 1.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
		assert!(Weighted::new(probability, "branch").is_err());
	}

	let impossible = Weighted::new(0.0, "impossible").expect("zero is a valid weight");
	let certain = Weighted::new(1.0, "certain").expect("one is a valid weight");
	assert_eq!(impossible.probability(), 0.0);
	assert_eq!(certain.probability(), 1.0);
	assert_eq!(certain.value(), &"certain");
}
