#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Matrix3f32 {
    pub m11: f32,
    pub m12: f32,
    pub m13: f32,
    pub m21: f32,
    pub m22: f32,
    pub m23: f32,
    pub m31: f32,
    pub m32: f32,
    pub m33: f32,
}

impl From<nalgebra::Matrix3<f32>> for Matrix3f32 {
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

impl From<Matrix3f32> for nalgebra::Matrix3<f32> {
    fn from(m: Matrix3f32) -> Self {
        Self::new(
            m.m11, m.m12, m.m13, m.m21, m.m22, m.m23, m.m31, m.m32, m.m33,
        )
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Matrix3x1f32 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl From<nalgebra::Matrix3x1<f32>> for Matrix3x1f32 {
    fn from(m: nalgebra::Matrix3x1<f32>) -> Self {
        Self {
            x: m.x,
            y: m.y,
            z: m.z,
        }
    }
}

impl From<Matrix3x1f32> for nalgebra::Vector3<f32> {
    fn from(m: Matrix3x1f32) -> Self {
        Self::new(m.x, m.y, m.z)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Vector3f32 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl From<nalgebra::Vector3<f32>> for Vector3f32 {
    fn from(v: nalgebra::Vector3<f32>) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}

impl From<Vector3f32> for nalgebra::Vector3<f32> {
    fn from(v: Vector3f32) -> Self {
        Self::new(v.x, v.y, v.z)
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Vector2f32 {
    pub x: f32,
    pub y: f32,
}

impl From<nalgebra::Vector2<f32>> for Vector2f32 {
    fn from(v: nalgebra::Vector2<f32>) -> Self {
        Self { x: v.x, y: v.y }
    }
}

impl From<Vector2f32> for nalgebra::Vector2<f32> {
    fn from(v: Vector2f32) -> Self {
        Self::new(v.x, v.y)
    }
}
