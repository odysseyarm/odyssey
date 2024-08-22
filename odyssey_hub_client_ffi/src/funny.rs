#[allow(unused)]
pub struct Client;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Matrix3f64 {
    pub m11: f64,
    pub m12: f64,
    pub m13: f64,
    pub m21: f64,
    pub m22: f64,
    pub m23: f64,
    pub m31: f64,
    pub m32: f64,
    pub m33: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Matrix3x1f64 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl From<nalgebra::Matrix3<f64>> for Matrix3f64 {
    fn from(m: nalgebra::Matrix3<f64>) -> Self {
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

impl From<nalgebra::Matrix3x1<f64>> for Matrix3x1f64 {
    fn from(m: nalgebra::Matrix3x1<f64>) -> Self {
        Self {
            x: m.x,
            y: m.y,
            z: m.z,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Vector3f64 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl From<nalgebra::Vector3<f64>> for Vector3f64 {
    fn from(v: nalgebra::Vector3<f64>) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Vector2f64 {
    pub x: f64,
    pub y: f64,
}

impl From<nalgebra::Vector2<f64>> for Vector2f64 {
    fn from(v: nalgebra::Vector2<f64>) -> Self {
        Self {
            x: v.x,
            y: v.y,
        }
    }
}
