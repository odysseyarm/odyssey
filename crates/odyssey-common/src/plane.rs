use nalgebra::{RealField, Scalar};
use num::FromPrimitive;
use num::traits::Float;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub struct Plane<F: Float + FromPrimitive + Scalar> {
    pub origin: nalgebra::Point3<F>,
    pub normal: nalgebra::Vector3<F>,
}

impl<F: Float + FromPrimitive + Scalar> Plane<F> {
    pub fn cast<G>(&self) -> Plane<G>
    where
        G: Float + FromPrimitive + Scalar + RealField,
        F: simba::scalar::SubsetOf<G>,
    {
        Plane {
            origin: self.origin.cast(),
            normal: self.normal.cast(),
        }
    }
}
