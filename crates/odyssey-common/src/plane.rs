use nalgebra::{RealField, Scalar};
use num::FromPrimitive;
use num::traits::Float;

/// Since: Odyssey Desktop 0.6.0.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Copy, Debug)]
pub struct Plane<F: Float + FromPrimitive + Scalar> {
    /// Since: Odyssey Desktop 0.6.0.
    pub origin: nalgebra::Point3<F>,
    /// Since: Odyssey Desktop 0.6.0.
    pub normal: nalgebra::Vector3<F>,
}

impl<F: Float + FromPrimitive + Scalar> Plane<F> {
    /// Since: Odyssey Desktop 0.6.0.
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
