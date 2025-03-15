use std::{cell::{Cell, RefCell}, iter::zip, marker::PhantomData};

use argmin::{core::{observers::ObserverMode, Executor, State}, solver::{gaussnewton::GaussNewton, gradientdescent::SteepestDescent, linesearch::{HagerZhangLineSearch, MoreThuenteLineSearch}, trustregion::{CauchyPoint, Dogleg, Steihaug, TrustRegion}}};
use argmin_observer_slog::SlogLogger;
use nalgebra::{allocator::Allocator, dvector, matrix, Const, DefaultAllocator, Dim, Dyn, Matrix, Matrix3, Matrix4, OMatrix, OVector, RealField, Rotation3, SMatrix, SVector, Storage, Unit, Vector, Vector2, Vector3, U1, U3};
use num_dual::{hessian, Dual2SVec64, Dual2Vec, DualNum, DualNumFloat, DualVec};

// k is the intrinsic matrix (fx, fy, cx, cy)
// e is the extrinsic matrix [r_1, r_2, t]
// q is the characteristic matrix of the ellipse
pub fn estimate_center<D: DualNum<f64>>(
    k: Matrix3<D>,
    e: Matrix3<D>,
    q_inv: Matrix3<D>,
) -> Vector2<D> {
    let p = k * e.clone() * q_inv * e.transpose() * Vector3::new(D::zero(), D::zero(), D::one());
    p.xy() / p.z.clone()
}

pub fn make_extrinsics<D: DualNum<f64> + RealField>(r: Vector3<D>, t: Vector3<D>) -> Matrix3<D> {
    let r = rotation_from_scaled_axis(r);
    let mut e = Matrix3::identity();
    e.fixed_view_mut::<3, 2>(0, 0)
        .copy_from(&r.matrix().fixed_view::<3, 2>(0, 0));
    e.fixed_view_mut::<3, 1>(0, 2)
        .copy_from(&t);
    e
}

pub fn make_extrinsics_from_mat4<D: DualNum<f64> + RealField>(m: &Matrix4<D>) -> Matrix3<D> {
    let mut e = Matrix3::identity();
    e.fixed_view_mut::<3, 2>(0, 0)
        .copy_from(&m.fixed_view::<3, 2>(0, 0));
    e.fixed_view_mut::<3, 1>(0, 2)
        .copy_from(&m.fixed_view::<3, 1>(0, 3));
    e
}

pub fn reproject<D: DualNum<f64> + RealField>(
    fx: D,
    fy: D,
    cx: D,
    cy: D,
    r: Vector3<D>,
    t: Vector3<D>,
    object_point: Vector3<f64>,
    circle_radius: f64,
) -> Vector2<D> {
    let _0 = || D::zero();
    let _1 = || D::one();
    let k = matrix![
        fx,   _0(),   cx;
        _0(),   fy,   cy;
        _0(), _0(), _1();
    ];
    let e = make_extrinsics(r, t) * matrix![
        circle_radius, 0., object_point.x;
        0., circle_radius, object_point.y;
        0., 0., 1.;
    ].cast();
    let q_inv = Matrix3::from_diagonal(&[1., 1., -1.].into());
    estimate_center(k, e, q_inv.cast())
}

pub fn reprojection_error<D: DualNum<f64> + RealField>(
    fx: D,
    fy: D,
    cx: D,
    cy: D,
    r: Vector3<D>,
    t: Vector3<D>,
    object_point: Vector3<f64>,
    image_point: Vector2<f64>,
    circle_radius: f64,
) -> Vector2<D> {
    let reproj = reproject(fx, fy, cx, cy, r, t, object_point, circle_radius);
    reproj - image_point.cast()
}

pub fn angular_reprojection_error<D: DualNum<f64> + RealField>(
    fx: D,
    fy: D,
    cx: D,
    cy: D,
    r: Vector3<D>,
    t: Vector3<D>,
    object_point: Vector3<f64>,
    image_point: Vector2<f64>,
    circle_radius: f64,
) -> D {
    let reproj = reproject(fx, fy, cx, cy, r, t, object_point, circle_radius);
    reproj.push(1.0.into()).angle(&image_point.push(1.0).cast())
}

pub fn calibrate<D: Dim, S: Storage<f64, Const<2>, D>>(
    object_points: &[Vector3<f64>],
    images_points: &[Matrix<f64, Const<2>, D, S>],
    circle_radius: f64,
    initial_focal_length: f64,
    initial_cx: f64,
    initial_cy: f64,
    initial_target_pos: Vector3<f64>, // initial guess for the target's position in camera space
) -> OVector<f64, Dyn> {
    for img_p in images_points {
        assert_eq!(object_points.len(), img_p.ncols());
    }

    // let cost = |v: OVector<Dual2Vec<f64, f64, Dyn>, Dyn>| {
    //     let mut cost = Dual2Vec::<f64, f64, _>::from_re(0.0);
    //     let [fx, fy, cx, cy, extrinsics @ ..] = v.as_slice() else { unreachable!() };
    //     for (image_points, e) in zip(images_points, extrinsics.chunks_exact(6)) {
    //         for (obj_p, img_p) in zip(object_points, image_points.column_iter()) {
    //             let fx = fx.clone();
    //             let fy = fy.clone();
    //             let cx = cx.clone();
    //             let cy = cy.clone();
    //             let r = Vector3::new(e[0].clone(), e[1].clone(), e[2].clone());
    //             let t = Vector3::new(e[3].clone(), e[4].clone(), e[5].clone());
    //             cost += reprojection_error(fx, fy, cx, cy, r, t, *obj_p, img_p.into(), circle_radius).norm_squared();
    //         }
    //     }
    //     cost
    // };
    // let problem = Problem {
    //     f: cost,
    //     last_param: None.into(),
    //     last_value: None.into(),
    //     last_gradient: None.into(),
    //     last_hessian: None.into(),
    // };
    let cost = |v: OVector<DualVec<f64, f64, Dyn>, Dyn>| {
        let mut residuals = dvector![];
        let [fx, fy, cx, cy, extrinsics @ ..] = v.as_slice() else { unreachable!() };
        for (image_points, e) in zip(images_points, extrinsics.chunks_exact(6)) {
            for (obj_p, img_p) in zip(object_points, image_points.column_iter()) {
                let fx = fx.clone();
                let fy = fy.clone();
                let cx = cx.clone();
                let cy = cy.clone();
                let r = Vector3::new(e[0].clone(), e[1].clone(), e[2].clone());
                let t = Vector3::new(e[3].clone(), e[4].clone(), e[5].clone());
                let [[x, y]] = reprojection_error(fx, fy, cx, cy, r, t, *obj_p, img_p.into(), circle_radius).data.0;
                residuals = residuals.push(x);
                residuals = residuals.push(y);
            }
        }
        residuals
    };
    let problem = Problem2 {
        f: cost,
        last_param: None.into(),
        last_value: None.into(),
        last_gradient: None.into(),
        last_hessian: None.into(),
    };

    let mut initial_params = vec![0.0; 4 + 6*images_points.len()];
    initial_params[0] = initial_focal_length;
    initial_params[1] = initial_focal_length;
    initial_params[2] = initial_cx;
    initial_params[3] = initial_cy;
    for ex in initial_params[4..].chunks_exact_mut(6) {
        ex[3] = initial_target_pos.x;
        ex[4] = initial_target_pos.y;
        ex[5] = initial_target_pos.z;
    }
    // let trust_region = TrustRegion::new(Dogleg::new());
    let trust_region = TrustRegion::new(Steihaug::new().with_max_iters(20));
    let result = Executor::new(problem, trust_region)
        .configure(|state| state
            .param(initial_params.into())
            .max_iters(10000)
            .target_cost(0.0)
        )
        .add_observer(SlogLogger::term(), ObserverMode::Every(1000))
        .run()
        .expect("optimization failed");

    // refine with angular error
    let cost2 = |v: OVector<DualVec<f64, f64, Dyn>, Dyn>| {
        let mut residuals = dvector![];
        let [fx, fy, cx, cy, extrinsics @ ..] = v.as_slice() else { unreachable!() };
        for (image_points, e) in zip(images_points, extrinsics.chunks_exact(6)) {
            for (obj_p, img_p) in zip(object_points, image_points.column_iter()) {
                let fx = fx.clone();
                let fy = fy.clone();
                let cx = cx.clone();
                let cy = cy.clone();
                let r = Vector3::new(e[0].clone(), e[1].clone(), e[2].clone());
                let t = Vector3::new(e[3].clone(), e[4].clone(), e[5].clone());
                let res = angular_reprojection_error(fx, fy, cx, cy, r, t, *obj_p, img_p.into(), circle_radius);
                residuals = residuals.push(res);
            }
        }
        residuals
    };
    let problem2 = Problem2 {
        f: cost2,
        last_param: None.into(),
        last_value: None.into(),
        last_gradient: None.into(),
        last_hessian: None.into(),
    };
    let result = Executor::new(problem2, TrustRegion::new(Steihaug::new().with_max_iters(20)))
        .configure(|state| state
            .param(result.state.best_param.unwrap())
            .max_iters(10000)
            .target_cost(0.0)
        )
        .add_observer(SlogLogger::term(), ObserverMode::NewBest)
        .run()
        .expect("optimization failed");

    let status = result.state.get_termination_status().clone();
    let best_param = result.state.best_param.unwrap();
    let [fx, fy, cx, cy, ..] = best_param.as_slice() else { unreachable!() };
    let cost = result.state.best_cost;
    println!("Converged after {} iterations", result.state.iter);
    println!("Termination status: {status}");
    println!("fx: {fx}");
    println!("fy: {fy}");
    println!("cx: {cx}");
    println!("cy: {cy}");
    println!("cost: {cost:?}");
    println!();
    best_param
}

struct Problem<G, D>
where
    D: Dim,
    G: Fn(OVector<Dual2Vec<f64, f64, D>, D>) -> Dual2Vec<f64, f64, D>,
    DefaultAllocator: Allocator<D> + Allocator<U1, D> + Allocator<D, D>,
{
    f: G,
    last_param: RefCell<Option<OVector<f64, D>>>,
    last_value: RefCell<Option<f64>>,
    last_gradient: RefCell<Option<OVector<f64, D>>>,
    last_hessian: RefCell<Option<OMatrix<f64, D, D>>>,
}

impl<G, D> Problem<G, D>
where
    D: Dim,
    G: Fn(OVector<Dual2Vec<f64, f64, D>, D>) -> Dual2Vec<f64, f64, D>,
    DefaultAllocator: Allocator<D> + Allocator<U1, D> + Allocator<D, D>,
{
    fn evaluate(&self, param: &OVector<f64, D>) {
        let mut last_param = self.last_param.borrow_mut();
        if last_param.as_ref() != Some(param) {
            let (value, gradient, hessian) = num_dual::hessian(&self.f, param.clone());
            *last_param = Some(param.clone());
            *self.last_value.borrow_mut() = Some(value);
            *self.last_gradient.borrow_mut() = Some(gradient);
            *self.last_hessian.borrow_mut() = Some(hessian);
        }
    }
}

impl<G, D> argmin::core::CostFunction for Problem<G, D>
where
    D: Dim,
    G: Fn(OVector<Dual2Vec<f64, f64, D>, D>) -> Dual2Vec<f64, f64, D>,
    DefaultAllocator: Allocator<D> + Allocator<U1, D> + Allocator<D, D>,
{
    type Param = OVector<f64, D>;
    type Output = f64;

    fn cost(&self, param: &Self::Param) -> Result<Self::Output, argmin_math::Error> {
        self.evaluate(param);
        Ok(self.last_value.borrow().clone().unwrap())
    }
}

impl<G, D> argmin::core::Gradient for Problem<G, D>
where
    D: Dim,
    G: Fn(OVector<Dual2Vec<f64, f64, D>, D>) -> Dual2Vec<f64, f64, D>,
    DefaultAllocator: Allocator<D> + Allocator<U1, D> + Allocator<D, D>,
{
    type Param = OVector<f64, D>;
    type Gradient = OVector<f64, D>;

    fn gradient(&self, param: &Self::Param) -> Result<Self::Gradient, argmin_math::Error> {
        self.evaluate(param);
        Ok(self.last_gradient.borrow().clone().unwrap())
    }
}

impl<G, D> argmin::core::Hessian for Problem<G, D>
where
    D: Dim,
    G: Fn(OVector<Dual2Vec<f64, f64, D>, D>) -> Dual2Vec<f64, f64, D>,
    DefaultAllocator: Allocator<D> + Allocator<U1, D> + Allocator<D, D>,
{
    type Param = OVector<f64, D>;
    type Hessian = OMatrix<f64, D, D>;

    fn hessian(&self, param: &Self::Param) -> Result<Self::Hessian, argmin_math::Error> {
        self.evaluate(param);
        Ok(self.last_hessian.borrow().clone().unwrap())
    }
}

#[derive(Clone)]
struct Problem2<G, M, N>
where
    M: Dim,
    N: Dim,
    G: Fn(OVector<DualVec<f64, f64, N>, N>) -> OVector<DualVec<f64, f64, N>, M>,
    DefaultAllocator: Allocator<M> + Allocator<N> + Allocator<N, N> + Allocator<nalgebra::Const<1>, N>,
{
    f: G,
    last_param: RefCell<Option<OVector<f64, N>>>,
    last_value: RefCell<Option<f64>>,
    last_gradient: RefCell<Option<OVector<f64, N>>>,
    last_hessian: RefCell<Option<OMatrix<f64, N, N>>>,
}


impl<G, M, N> Problem2<G, M, N>
where
    M: Dim,
    N: Dim,
    G: Fn(OVector<DualVec<f64, f64, N>, N>) -> OVector<DualVec<f64, f64, N>, M>,
    DefaultAllocator: Allocator<M> + Allocator<N> + Allocator<M, N> + Allocator<N, M> + Allocator<N, N> + Allocator<U1, N>,
{
    fn evaluate(&self, param: &OVector<f64, N>) {
        let mut last_param = self.last_param.borrow_mut();
        if last_param.as_ref() != Some(param) {
            let (value, jacobian) = num_dual::jacobian(&self.f, param.clone());

            let jacobian_t = jacobian.transpose();
            let gradient = 2.0*jacobian_t.clone()*value.clone();
            let hessian = 2.0 * jacobian_t * jacobian;
            *last_param = Some(param.clone());
            *self.last_value.borrow_mut() = Some(value.norm_squared());
            *self.last_gradient.borrow_mut() = Some(gradient);
            *self.last_hessian.borrow_mut() = Some(hessian);
        }
    }
}

impl<G, M, N> argmin::core::CostFunction for Problem2<G, M, N>
where
    M: Dim,
    N: Dim,
    G: Fn(OVector<DualVec<f64, f64, N>, N>) -> OVector<DualVec<f64, f64, N>, M>,
    DefaultAllocator: Allocator<M> + Allocator<N> + Allocator<M, N> + Allocator<N, M> + Allocator<N, N> + Allocator<U1, N>,
{
    type Param = OVector<f64, N>;
    type Output = f64;

    fn cost(&self, param: &Self::Param) -> Result<Self::Output, argmin_math::Error> {
        self.evaluate(param);
        Ok(self.last_value.borrow().clone().unwrap())
    }
}

impl<G, M, N> argmin::core::Gradient for Problem2<G, M, N>
where
    M: Dim,
    N: Dim,
    G: Fn(OVector<DualVec<f64, f64, N>, N>) -> OVector<DualVec<f64, f64, N>, M>,
    DefaultAllocator: Allocator<M> + Allocator<N> + Allocator<M, N> + Allocator<N, M> + Allocator<N, N> + Allocator<U1, N>,
{
    type Param = OVector<f64, N>;
    type Gradient = OVector<f64, N>;

    fn gradient(&self, param: &Self::Param) -> Result<Self::Gradient, argmin_math::Error> {
        self.evaluate(param);
        Ok(self.last_gradient.borrow().clone().unwrap())
    }
}

impl<G, M, N> argmin::core::Hessian for Problem2<G, M, N>
where
    M: Dim,
    N: Dim,
    G: Fn(OVector<DualVec<f64, f64, N>, N>) -> OVector<DualVec<f64, f64, N>, M>,
    DefaultAllocator: Allocator<M> + Allocator<N> + Allocator<M, N> + Allocator<N, M> + Allocator<N, N> + Allocator<U1, N>,
{
    type Param = OVector<f64, N>;
    type Hessian = OMatrix<f64, N, N>;

    fn hessian(&self, param: &Self::Param) -> Result<Self::Hessian, argmin_math::Error> {
        self.evaluate(param);
        Ok(self.last_hessian.borrow().clone().unwrap())
    }
}

/// Custom special case at 0
pub fn rotation_from_scaled_axis<T, SB>(axisangle: Vector<T, U3, SB>) -> Rotation3<T>
where
    T: nalgebra::SimdRealField,
    T::Element: nalgebra::SimdRealField,

    SB: Storage<T, U3>,
{
    let a = axisangle.into_owned();
    let (axis, angle) = Unit::new_and_get(a.clone());
    if angle.is_zero() {
        let _1 = T::one;
        Rotation3::from_matrix_unchecked(matrix![
            _1(), -a.z.clone(),  a.y.clone();
             a.z.clone(), _1(), -a.x.clone();
            -a.y.clone(),  a.x.clone(), _1();
        ])
    } else {
        Rotation3::from_axis_angle(&axis, angle)
    }
}
