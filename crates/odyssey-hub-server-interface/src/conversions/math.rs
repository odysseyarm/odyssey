use crate::{Matrix3x1, Matrix3x3, Vector2, Vector3};

impl From<nalgebra::Matrix3x1<f32>> for Matrix3x1 {
    fn from(v: nalgebra::Matrix3x1<f32>) -> Self {
        Self {
            m11: v.x,
            m21: v.y,
            m31: v.z,
        }
    }
}
impl From<Matrix3x1> for nalgebra::Matrix3x1<f32> {
    fn from(v: Matrix3x1) -> Self {
        Self::new(v.m11, v.m21, v.m31)
    }
}

impl From<nalgebra::Matrix3<f32>> for Matrix3x3 {
    fn from(m: nalgebra::Matrix3<f32>) -> Self {
        Self {
            m11: m.m11,
            m12: m.m12,
            m13: m.m13,
            m21: m.m21,
            m22: m.m22,
            m23: m.m23,
            m31: m.m31,
            m32: m.m32,
            m33: m.m33,
        }
    }
}
impl From<Matrix3x3> for nalgebra::Matrix3<f32> {
    fn from(m: Matrix3x3) -> Self {
        Self::new(
            m.m11, m.m12, m.m13, m.m21, m.m22, m.m23, m.m31, m.m32, m.m33,
        )
    }
}

impl From<nalgebra::Vector2<f32>> for Vector2 {
    fn from(v: nalgebra::Vector2<f32>) -> Self {
        Self { x: v.x, y: v.y }
    }
}
impl From<Vector2> for nalgebra::Vector2<f32> {
    fn from(v: Vector2) -> Self {
        Self::new(v.x, v.y)
    }
}

impl From<nalgebra::Vector3<f32>> for Vector3 {
    fn from(v: nalgebra::Vector3<f32>) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}
impl From<Vector3> for nalgebra::Vector3<f32> {
    fn from(v: Vector3) -> Self {
        Self::new(v.x, v.y, v.z)
    }
}
