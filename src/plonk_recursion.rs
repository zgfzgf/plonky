use crate::gates::evaluate_all_constraints_recursively;
use crate::plonk_challenger::RecursiveChallenger;
use crate::plonk_proof::OldProofTarget;
use crate::plonk_util::{powers_recursive, reduce_with_powers_recursive};
use crate::util::ceil_div_usize;
use crate::{get_subgroup_shift, hash_usize_to_curve, AffinePointTarget, Circuit, CircuitBuilder, CurveMulEndoResult, CurveMulOp, Field, HaloCurve, OpeningSetTarget, ProofTarget, SchnorrProofTarget, Target, GRID_WIDTH, NUM_CONSTANTS, NUM_ROUTED_WIRES, NUM_WIRES, QUOTIENT_POLYNOMIAL_DEGREE_MULTIPLIER};

/// Wraps a `Circuit` for recursive verification with inputs for the proof data.
/// The circuit is over the field `C::ScalarField` and verifies a proof performed over the curve `InnerC`.
pub struct RecursiveCircuit<C: HaloCurve, InnerC: HaloCurve<BaseField = C::ScalarField>> {
    pub circuit: Circuit<C>,
    pub public_inputs: RecursionPublicInputs<C::ScalarField>,
    pub proof: ProofTarget<C, InnerC>,
    pub old_proofs: Vec<OldProofTarget<InnerC>>,
}

/// Public inputs of the recursive circuit. This contains data for the inner proof which is needed
/// to complete verification of it.
pub struct RecursionPublicInputs<F: Field> {
    beta: Target<F>,
    gamma: Target<F>,
    alpha: Target<F>,
    zeta: Target<F>,
    o_constants: Vec<Target<F>>,
    o_plonk_sigmas: Vec<Target<F>>,
    o_local_wires: Vec<Target<F>>,
    o_right_wires: Vec<Target<F>>,
    o_below_wires: Vec<Target<F>>,
    o_plonk_z_local: Target<F>,
    o_plonk_z_right: Target<F>,
    o_plonk_t: Vec<Target<F>>,
    halo_us: Vec<Target<F>>,
    old_proofs: Vec<Target<F>>,
}

/// The number of `PublicInputGate`s needed to route the given number of public inputs.
fn num_public_input_gates(num_public_inputs: usize) -> usize {
    ceil_div_usize(num_public_inputs, NUM_WIRES)
}

pub fn recursive_verification_circuit<
    C: HaloCurve,
    InnerC: HaloCurve<BaseField = C::ScalarField>,
>(
    degree_pow: usize,
    security_bits: usize,
    num_public_inputs: usize,
    num_old_proofs: usize,
) -> RecursiveCircuit<C, InnerC> {
    let mut builder = CircuitBuilder::<C>::new(security_bits);
    let public_inputs = RecursionPublicInputs {
        beta: builder.add_public_input(),
        gamma: builder.add_public_input(),
        alpha: builder.add_public_input(),
        zeta: builder.add_public_input(),
        o_constants: builder.add_public_inputs(NUM_CONSTANTS),
        o_plonk_sigmas: builder.add_public_inputs(NUM_ROUTED_WIRES),
        o_local_wires: builder.add_public_inputs(NUM_WIRES),
        o_right_wires: builder.add_public_inputs(NUM_WIRES),
        o_below_wires: builder.add_public_inputs(NUM_WIRES),
        o_plonk_z_local: builder.add_public_input(),
        o_plonk_z_right: builder.add_public_input(),
        o_plonk_t: builder.add_public_inputs(QUOTIENT_POLYNOMIAL_DEGREE_MULTIPLIER),
        halo_us: builder.add_public_inputs(degree_pow),
        old_proofs: builder.add_public_inputs((2 + degree_pow) * num_old_proofs),
    };

    let num_public_input_gates = num_public_input_gates(num_public_inputs);

    let proof = ProofTarget::<C, InnerC> {
        c_wires: builder.add_virtual_point_targets(NUM_WIRES),
        c_plonk_z: builder.add_virtual_point_target(),
        c_plonk_t: builder.add_virtual_point_targets(QUOTIENT_POLYNOMIAL_DEGREE_MULTIPLIER),
        o_public_inputs: Some(make_opening_sets(
            &mut builder,
            num_public_input_gates,
            num_old_proofs,
        )),
        o_local: make_opening_set(&mut builder, num_old_proofs),
        o_right: make_opening_set(&mut builder, num_old_proofs),
        o_below: make_opening_set(&mut builder, num_old_proofs),
        halo_l_i: builder.add_virtual_point_targets(degree_pow),
        halo_r_i: builder.add_virtual_point_targets(degree_pow),
        halo_g: builder.add_virtual_point_target(),
        schnorr_proof: make_schnorr_proof(&mut builder),
    };

    let old_proofs = make_old_proofs(&mut builder, num_old_proofs, degree_pow);

    // Flatten the list of public input openings.
    let o_public_inputs: Option<Vec<Target<C::ScalarField>>> =
        proof.o_public_inputs.as_ref().map(|pis| {
            pis.iter()
                .cloned()
                .flat_map(|opening_set| opening_set.o_wires)
                .collect()
        });

    verify_assumptions::<C, InnerC>(
        &mut builder,
        degree_pow,
        &public_inputs,
        // TODO: Fix this.
        &o_public_inputs.unwrap(),
    );

    // TODO: Verify that each prover polynomial commitment is on the curve.
    // Can call curve_assert_valid.

    // Compute random challenges.
    let mut challenger = RecursiveChallenger::<C::ScalarField>::new(&mut builder);
    challenger.observe_affine_points(&proof.c_wires);
    let (beta, gamma) = challenger.get_2_challenges(&mut builder);
    challenger.observe_affine_point(proof.c_plonk_z);
    let alpha = challenger.get_challenge(&mut builder);
    challenger.observe_affine_points(&proof.c_plonk_t);
    let zeta = challenger.get_challenge(&mut builder);
    challenger.observe_elements(&proof.all_opening_targets());

    verify_old_proof_evaluation::<C, InnerC>(&mut builder, &old_proofs, &proof.o_local, zeta);

    // v: Challenge to combine different opening points.
    // u: Challenge to combine different polynomials.
    // u_scaling: Challenge for scaling u used in the IPA verification.
    let (v, u, u_scaling) = challenger.get_3_challenges(&mut builder);

    // Compute IPA challenges.
    let mut raw_ipa_challenges = Vec::new();
    for i in 0..degree_pow {
        challenger.observe_affine_points(&[proof.halo_l_i[i], proof.halo_r_i[i]]);
        let r = challenger.get_challenge(&mut builder);
        raw_ipa_challenges.push(r);
    }

    // Compute challenge for Schnorr protocol.
    let schnorr_challenge = challenger.get_challenge(&mut builder);

    let halo_us = verify_all_ipas::<C, InnerC>(
        &mut builder,
        &proof,
        zeta,
        u,
        v,
        u_scaling,
        raw_ipa_challenges,
        schnorr_challenge,
        num_public_input_gates,
        security_bits,
    );

    // "Outputs" data relating to assumption which still need to be verified by the next proof.
    builder.copy(public_inputs.beta, beta);
    builder.copy(public_inputs.gamma, gamma);
    builder.copy(public_inputs.alpha, alpha);
    builder.copy(public_inputs.zeta, zeta);
    for i in 0..NUM_CONSTANTS {
        builder.copy(
            public_inputs.o_constants[i],
            proof.o_local.o_constants[i],
        );
    }
    for i in 0..NUM_ROUTED_WIRES {
        builder.copy(
            public_inputs.o_plonk_sigmas[i],
            proof.o_local.o_plonk_sigmas[i],
        );
    }
    for i in 0..NUM_WIRES {
        builder.copy(
            public_inputs.o_local_wires[i],
            proof.o_local.o_wires[i],
        );
        builder.copy(
            public_inputs.o_right_wires[i],
            proof.o_right.o_wires[i],
        );
        builder.copy(
            public_inputs.o_below_wires[i],
            proof.o_below.o_wires[i],
        );
    }
    builder.copy(
        public_inputs.o_plonk_z_local,
        proof.o_local.o_plonk_z,
    );
    builder.copy(
        public_inputs.o_plonk_z_right,
        proof.o_right.o_plonk_z,
    );
    for i in 0..degree_pow {
        builder.copy(public_inputs.halo_us[i], halo_us[i]);
    }
    let shift = 2 + degree_pow;
    for i in 0..num_old_proofs {
        builder.copy(
            old_proofs[i].halo_g.x,
            public_inputs.old_proofs[shift * i],
        );
        builder.copy(
            old_proofs[i].halo_g.y,
            public_inputs.old_proofs[shift * i + 1],
        );
        for j in 0..degree_pow {
            builder.copy(
                old_proofs[i].halo_us[j].convert(),
                public_inputs.old_proofs[shift * i + j + 2],
            );
        }
    }

    let circuit = builder.build();
    RecursiveCircuit {
        circuit,
        public_inputs,
        proof,
        old_proofs,
    }
}

/// Verify all IPAs in the given proof, and return IPA challenges.
#[allow(clippy::too_many_arguments)]
fn verify_all_ipas<C: HaloCurve, InnerC: HaloCurve<BaseField = C::ScalarField>>(
    builder: &mut CircuitBuilder<C>,
    proof: &ProofTarget<C, InnerC>,
    zeta: Target<C::ScalarField>,
    u: Target<C::ScalarField>,
    v: Target<C::ScalarField>,
    u_scaling: Target<C::ScalarField>,
    raw_ipa_challenges: Vec<Target<C::ScalarField>>,
    schnorr_challenge: Target<C::ScalarField>,
    num_public_input_gates: usize,
    security_bits: usize,
) -> Vec<Target<C::ScalarField>> {
    // Reduce all polynomial commitments to a single one, i.e. a random combination of them.
    // TODO: Configure the actual constants and permutations of whatever circuit we wish to verify.
    // For now, we use a dummy point for each of those polynomial commitments.
    let dummy_point = builder.constant_affine_point(InnerC::GENERATOR_AFFINE);
    let c_constants = vec![dummy_point; NUM_CONSTANTS];
    let c_s_sigmas = vec![dummy_point; NUM_ROUTED_WIRES];
    let c_all: Vec<AffinePointTarget<InnerC>> = [
        c_constants,
        c_s_sigmas,
        proof.c_wires.clone(),
        vec![proof.c_plonk_z],
        proof.c_plonk_t.clone(),
    ]
    .concat();
    let mut c_reduction_muls = Vec::new();
    let powers_of_u = powers_recursive(builder, u, c_all.len());
    for (&c, &power) in c_all.iter().zip(powers_of_u.iter()) {
        let mul = CurveMulOp {
            scalar: power,
            point: c,
        };
        c_reduction_muls.push(mul);
    }
    let c_reduction_msm_result = builder.curve_msm_endo::<InnerC>(&c_reduction_muls);
    let actual_scalars = c_reduction_msm_result.actual_scalars;
    let c_reduction = c_reduction_msm_result.msm_result;

    // For each opening set, we do a similar reduction, using the actual scalars above.
    let opening_set_reductions: Vec<Target<C::ScalarField>> = proof
        .all_opening_sets()
        .iter()
        .map(|opening_set| {
            reduce_with_coefficients(builder, &opening_set.to_vec(), &actual_scalars)
        })
        .collect();

    // Then, we reduce the above opening set reductions to a single value.
    let reduced_opening = reduce_with_powers_recursive(builder, &opening_set_reductions, v);

    // u is set to H(degree + 1).
    let u = builder.constant_affine_point(hash_usize_to_curve::<InnerC>(
        (1 << raw_ipa_challenges.len()) + 1,
        security_bits,
    ));
    // u' is u scaled by n(u_scaling), giving a random generator.
    let u_prime = builder
        .curve_mul_endo::<InnerC>(CurveMulOp {
            scalar: u_scaling,
            point: u,
        })
        .mul_result;

    // Compute [v] u', where v is the (reduced) opened value.
    let v_u_prime = builder.curve_mul::<InnerC>(CurveMulOp {
        scalar: reduced_opening,
        point: u_prime,
    });
    let p_prime = builder.curve_add::<InnerC>(c_reduction, v_u_prime);

    let (halo_q, halo_us) =
        compute_halo_q::<C, InnerC>(builder, proof, &raw_ipa_challenges, p_prime);

    let generator_n_value = C::ScalarField::primitive_root_of_unity(raw_ipa_challenges.len());
    let generator_n = builder.constant_wire(generator_n_value);
    let mut points: Vec<Target<C::ScalarField>> = (0..2 * num_public_input_gates)
        .scan(C::ScalarField::ONE, |acc, _| {
            let tmp = *acc;
            *acc = *acc * generator_n_value;
            Some(tmp)
        })
        .step_by(2)
        .map(|f| builder.constant_wire(f))
        .collect();
    points.extend(&[zeta, builder.mul(zeta, generator_n), {
        let g65 = builder.exp_constant_usize(generator_n, GRID_WIDTH);
        builder.mul(zeta, g65)
    }]);
    let halo_bs = points
        .into_iter()
        .map(|p| halo_g_recursive(builder, p, &halo_us))
        .collect::<Vec<_>>();
    let halo_b = reduce_with_powers_recursive(builder, &halo_bs, v);

    verify_schnorr::<C, InnerC>(
        builder,
        proof,
        u_prime,
        halo_q,
        halo_b,
        schnorr_challenge,
        security_bits,
    );

    halo_us
}

/// Computes `Q` in the context of the Halo paper. Returns `(Q, halo_us)`.
fn compute_halo_q<C: HaloCurve, InnerC: HaloCurve<BaseField = C::ScalarField>>(
    builder: &mut CircuitBuilder<C>,
    proof: &ProofTarget<C, InnerC>,
    raw_ipa_challenges: &[Target<C::ScalarField>],
    p_prime: AffinePointTarget<InnerC>,
) -> (AffinePointTarget<InnerC>, Vec<Target<C::ScalarField>>) {
    let mut sum = p_prime;

    // The summation of the L_i terms has the structure of an MSM.
    let mut l_muls = Vec::new();
    for (i, &raw_ipa_challenge) in raw_ipa_challenges.iter().enumerate() {
        l_muls.push(CurveMulOp {
            scalar: raw_ipa_challenge,
            point: proof.halo_l_i[i],
        });
    }
    let l_msm_result = builder.curve_msm_endo::<InnerC>(&l_muls);
    sum = builder.curve_add::<InnerC>(sum, l_msm_result.msm_result);

    // For the R_i terms, there is no MSM structure because of the inverses. Add them one by one.
    for (i, &raw_ipa_challenge) in raw_ipa_challenges.iter().enumerate() {
        let CurveMulEndoResult {
            mul_result,
            actual_scalar,
        } = builder.curve_mul_inv_endo::<InnerC>(CurveMulOp {
            scalar: raw_ipa_challenge,
            point: proof.halo_r_i[i],
        });
        sum = builder.curve_add::<InnerC>(sum, mul_result);
        // Make sure the scalars used to scale L_i and R_i match.
        builder.copy(actual_scalar, l_msm_result.actual_scalars[i]);
    }

    let halo_us = l_msm_result
        .actual_scalars
        .iter()
        .map(|&n_r| builder.deterministic_square_root(n_r))
        .collect();

    (sum, halo_us)
}

/// Verify the final Schnorr protocol used in Halo.
fn verify_schnorr<C: HaloCurve, InnerC: HaloCurve<BaseField = C::ScalarField>>(
    builder: &mut CircuitBuilder<C>,
    proof: &ProofTarget<C, InnerC>,
    u_prime: AffinePointTarget<InnerC>,
    halo_q: AffinePointTarget<InnerC>,
    halo_b: Target<C::ScalarField>,
    schnorr_challenge: Target<C::ScalarField>,
    security_bits: usize,
) {
    // The H point used for blinding polynomial commitments. Set to H(degree).
    let pedersen_h = builder.constant_affine_point(hash_usize_to_curve::<InnerC>(
        1 << proof.halo_l_i.len(),
        security_bits,
    ));

    // Perform ZK opening protocol.
    // LHS is [schnorr_challenge] * q + schnorr_proof.r.
    // RHS is [schnorr_proof.z1] * (halo_g + [halo_b] * u') + [schnorr_proof.z2] * pedersen_h.
    let lhs = builder.curve_mul::<InnerC>(CurveMulOp {
        scalar: schnorr_challenge,
        point: halo_q,
    });
    let lhs = builder.curve_add::<InnerC>(lhs, proof.schnorr_proof.r);
    let rhs = builder.curve_mul::<InnerC>(CurveMulOp {
        scalar: halo_b,
        point: u_prime,
    });
    let rhs = builder.curve_add::<InnerC>(rhs, proof.halo_g);
    let rhs = builder.curve_mul::<InnerC>(CurveMulOp {
        scalar: proof.schnorr_proof.z1.convert(),
        point: rhs,
    });
    let tmp = builder.curve_mul::<InnerC>(CurveMulOp {
        scalar: proof.schnorr_proof.z2.convert(),
        point: pedersen_h,
    });
    let rhs = builder.curve_add::<InnerC>(rhs, tmp);
    // LHS should be equal to RHS.
    builder.copy_curve(lhs, rhs);
}

fn make_opening_set<C: HaloCurve>(
    builder: &mut CircuitBuilder<C>,
    num_old_proofs: usize,
) -> OpeningSetTarget<C> {
    OpeningSetTarget {
        o_constants: builder.add_virtual_targets(NUM_CONSTANTS),
        o_plonk_sigmas: builder.add_virtual_targets(NUM_ROUTED_WIRES),
        o_wires: builder.add_virtual_targets(NUM_WIRES),
        o_plonk_z: builder.add_virtual_target(),
        o_plonk_t: builder.add_virtual_targets(QUOTIENT_POLYNOMIAL_DEGREE_MULTIPLIER),
        o_old_proofs: builder.add_virtual_targets(num_old_proofs),
    }
}
fn make_opening_sets<C: HaloCurve>(
    builder: &mut CircuitBuilder<C>,
    n: usize,
    num_old_proofs: usize,
) -> Vec<OpeningSetTarget<C>> {
    (0..n)
        .map(|_i| make_opening_set(builder, num_old_proofs))
        .collect()
}

fn make_schnorr_proof<C: HaloCurve, InnerC: HaloCurve<BaseField = C::ScalarField>>(
    builder: &mut CircuitBuilder<C>,
) -> SchnorrProofTarget<InnerC> {
    SchnorrProofTarget {
        r: builder.add_virtual_point_target(),
        z1: builder.add_virtual_target().convert(),
        z2: builder.add_virtual_target().convert(),
    }
}

fn make_old_proofs<C: HaloCurve, InnerC: HaloCurve<BaseField = C::ScalarField>>(
    builder: &mut CircuitBuilder<C>,
    n: usize,
    degree_pow: usize,
) -> Vec<OldProofTarget<InnerC>> {
    (0..n)
        .map(|_| OldProofTarget {
            halo_g: builder.add_virtual_point_target(),
            halo_us: Target::convert_slice(&builder.add_virtual_targets(degree_pow)),
        })
        .collect()
}

/// In our recursion scheme, to avoid non-native field arithmetic, each proof in a recursive chain
/// only partially verifies its inner proof. It outputs various challenges and openings, and the
/// following proof is expected to verify constraints upon that data. This function performs those
/// final verification steps.
fn verify_assumptions<C: HaloCurve, InnerC: HaloCurve<BaseField = C::ScalarField>>(
    builder: &mut CircuitBuilder<C>,
    degree_pow: usize,
    public_inputs: &RecursionPublicInputs<C::ScalarField>,
    o_public_inputs: &[Target<C::ScalarField>],
) {
    let degree = 1 << degree_pow;
    let degree_f = C::ScalarField::from_canonical_usize(degree);
    let degree_wire = builder.constant_wire(degree_f);

    let one = builder.one_wire();

    // Convert inner proof data from `PublicInput`s to `Target`s.
    let o_constants: Vec<Target<C::ScalarField>> = public_inputs
        .o_constants
        .iter()
        .map(|pi| o_public_inputs[pi.index()])
        .collect();
    let o_sigmas: Vec<Target<C::ScalarField>> = public_inputs
        .o_plonk_sigmas
        .iter()
        .map(|pi| o_public_inputs[pi.index()])
        .collect();
    let o_local_wires: Vec<Target<C::ScalarField>> = public_inputs
        .o_local_wires
        .iter()
        .map(|pi| o_public_inputs[pi.index()])
        .collect();
    let o_right_wires: Vec<Target<C::ScalarField>> = public_inputs
        .o_right_wires
        .iter()
        .map(|pi| o_public_inputs[pi.index()])
        .collect();
    let o_below_wires: Vec<Target<C::ScalarField>> = public_inputs
        .o_below_wires
        .iter()
        .map(|pi| o_public_inputs[pi.index()])
        .collect();
    let beta = o_public_inputs[public_inputs.beta.index()];
    let gamma = o_public_inputs[public_inputs.gamma.index()];
    let alpha = o_public_inputs[public_inputs.alpha.index()];
    let zeta = o_public_inputs[public_inputs.zeta.index()];
    let o_z_local = o_public_inputs[public_inputs.o_plonk_z_local.index()];
    let o_z_right = o_public_inputs[public_inputs.o_plonk_z_right.index()];

    // Evaluate zeta^degree.
    let mut zeta_power_d = zeta;
    for _i in 0..degree_pow {
        zeta_power_d = builder.double(zeta_power_d);
    }

    // Evaluate Z_H(zeta) = zeta^degree - 1.
    let zero_eval = builder.sub(zeta_power_d, one);

    // Evaluate L_1(zeta) = (zeta^degree - 1) / (degree * (zeta - 1)).
    let zeta_minus_one = builder.sub(zeta, one);
    let lagrange_1_eval_denominator = builder.mul(degree_wire, zeta_minus_one);
    let lagrange_1_eval = builder.div(zero_eval, lagrange_1_eval_denominator);

    // Compute Z(zeta) f'(zeta) - Z(g * zeta) g'(zeta), which should vanish on H.
    let mut f_prime = one;
    let mut g_prime = one;
    for i in 0..NUM_ROUTED_WIRES {
        let k_i = builder.constant_wire(get_subgroup_shift::<C::ScalarField>(i));
        let s_id = builder.mul(k_i, zeta);
        let beta_s_id = builder.mul(beta, s_id);
        let beta_s_sigma = builder.mul(beta, o_sigmas[i]);
        let f_prime_part = builder.add_many(&[o_local_wires[i], beta_s_id, gamma]);
        let g_prime_part = builder.add_many(&[o_local_wires[i], beta_s_sigma, gamma]);
        f_prime = builder.mul(f_prime, f_prime_part);
        g_prime = builder.mul(g_prime, g_prime_part);
    }
    let z_f_prime = builder.mul(o_z_local, f_prime);
    let z_shifted_g_prime = builder.mul(o_z_right, g_prime);
    let vanishing_v_shift_term = builder.sub(z_f_prime, z_shifted_g_prime);

    // Evaluate the function which is supposed to vanish on H. It is a sum of several terms which
    // should vanish, each weighted by a different power of alpha.
    let o_z_minus_1 = builder.sub(o_z_local, one);
    let vanishing_z_1_term = builder.mul(o_z_minus_1, lagrange_1_eval);
    let constraint_terms = evaluate_all_constraints_recursively::<C, InnerC>(
        builder,
        &o_constants,
        &o_local_wires,
        &o_right_wires,
        &o_below_wires,
    );
    let vanishing_terms = [
        vec![vanishing_z_1_term],
        vec![vanishing_v_shift_term],
        constraint_terms,
    ]
    .concat();
    let vanishing_eval = reduce_with_powers_recursive(builder, &vanishing_terms, alpha);

    // Evaluate the quotient polynomial, and assert that it matches the prover's opening.
    let quotient_eval = builder.div(vanishing_eval, zero_eval);
    let t_components: Vec<Target<C::ScalarField>> = public_inputs
        .o_plonk_t
        .iter()
        .map(|pi| o_public_inputs[pi.index()])
        .collect();
    let o_plonk_t_eval = eval_composite_poly(builder, &t_components, zeta_power_d);
    builder.copy(quotient_eval, o_plonk_t_eval);
}

/// Computes a sum of terms weighted by the given coefficients.
fn reduce_with_coefficients<C: HaloCurve>(
    builder: &mut CircuitBuilder<C>,
    terms: &[Target<C::ScalarField>],
    coefficients: &[Target<C::ScalarField>],
) -> Target<C::ScalarField> {
    let mut reduction = builder.zero_wire();
    for (i, &term) in terms.iter().enumerate() {
        reduction = builder.mul_add(coefficients[i], term, reduction);
    }
    reduction
}

/// In Plonk, some polynomials are broken up into degree-d components. Given an evaluation of each
/// component at some point zeta, this function evaluates the composite polynomial at zeta.
fn eval_composite_poly<C: HaloCurve>(
    builder: &mut CircuitBuilder<C>,
    component_evals: &[Target<C::ScalarField>],
    zeta_power_d: Target<C::ScalarField>,
) -> Target<C::ScalarField> {
    reduce_with_powers_recursive(builder, component_evals, zeta_power_d)
}

/// Evaluate `g(X, {u_i})` as defined in the Halo paper.
fn halo_g_recursive<C: HaloCurve>(
    builder: &mut CircuitBuilder<C>,
    x: Target<C::ScalarField>,
    us: &[Target<C::ScalarField>],
) -> Target<C::ScalarField> {
    let mut product = builder.one_wire();
    let mut x_power = x;
    for &u_i in us.iter().rev() {
        let u_i_inv = builder.inv(u_i);
        let term = builder.mul_add(u_i, x_power, u_i_inv);
        product = builder.mul(product, term);
        x_power = builder.double(x_power);
    }
    product
}

fn verify_old_proof_evaluation<C: HaloCurve, InnerC: HaloCurve<BaseField = C::ScalarField>>(
    builder: &mut CircuitBuilder<C>,
    old_proofs: &[OldProofTarget<InnerC>],
    o_local: &OpeningSetTarget<C>,
    zeta: Target<C::ScalarField>,
) {
    for (i, p) in old_proofs.iter().enumerate() {
        let computed = halo_g_recursive(builder, zeta, &Target::convert_slice(&p.halo_us));
        builder.copy(computed, o_local.o_old_proofs[i]);
    }
}
