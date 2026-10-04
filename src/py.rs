//! Python conversions for the protocol types pyo3 cannot derive.
//!
//! A `#[pyclass]` complex enum generates a getter for every variant field, so
//! each payload type has to implement `IntoPyObject`/`FromPyObject`. The orphan
//! rule blocks implementing those for `heapless::String`/`heapless::Vec`, and
//! pyo3 deliberately refuses `Result` as a value (it means "raise"), hence
//! [`DeviceName`], [`List`] and [`Outcome`].

use pyo3::{
    Borrowed, Bound, FromPyObject, IntoPyObject, PyAny, PyErr, PyResult, Python,
    types::{PyAnyMethods, PyDict, PyDictMethods, PyList, PyString, PyStringMethods},
};

use crate::{DeviceName, GeneralConfig, List, Outcome};

impl<'py> IntoPyObject<'py> for DeviceName {
    type Target = PyString;
    type Output = Bound<'py, PyString>;
    type Error = core::convert::Infallible;

    fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
        Ok(PyString::new(py, self.as_str()))
    }
}

impl<'a, 'py> FromPyObject<'a, 'py> for DeviceName {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        let s = obj.cast::<PyString>()?;
        let s = s.to_cow()?;
        if s.len() > 32 {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "device name must be at most 32 bytes",
            ));
        }
        Ok(DeviceName::from_str_lossy(&s))
    }
}

impl<'py, T, const N: usize> IntoPyObject<'py> for List<T, N>
where
    T: IntoPyObject<'py>,
{
    type Target = PyList;
    type Output = Bound<'py, PyList>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        PyList::new(py, self.0)
    }
}

impl<'a, 'py, T, const N: usize> FromPyObject<'a, 'py> for List<T, N>
where
    T: pyo3::prelude::FromPyObjectOwned<'py>,
{
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        let mut out = heapless::Vec::new();
        for item in obj.try_iter()? {
            let item = item?;
            let item = item
                .extract::<T>()
                .map_err(|e| -> PyErr { Into::<PyErr>::into(e) })?;
            out.push(item).map_err(|_| {
                pyo3::exceptions::PyValueError::new_err("too many items for this message")
            })?;
        }
        Ok(List(out))
    }
}

impl<'py, T, E> IntoPyObject<'py> for Outcome<T, E>
where
    T: IntoPyObject<'py>,
    E: IntoPyObject<'py>,
{
    type Target = PyDict;
    type Output = Bound<'py, PyDict>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        use pyo3::IntoPyObjectExt;
        let dict = PyDict::new(py);
        match self.0 {
            Ok(v) => dict.set_item("Ok", v.into_bound_py_any(py)?)?,
            Err(e) => dict.set_item("Err", e.into_bound_py_any(py)?)?,
        }
        Ok(dict)
    }
}

impl<'a, 'py, T, E> FromPyObject<'a, 'py> for Outcome<T, E>
where
    T: pyo3::prelude::FromPyObjectOwned<'py>,
    E: pyo3::prelude::FromPyObjectOwned<'py>,
{
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        let dict = obj.cast::<PyDict>()?;
        if let Some(ok) = dict.get_item("Ok")? {
            return Ok(Outcome(Ok(ok
                .extract::<T>()
                .map_err(|e| -> PyErr { Into::<PyErr>::into(e) })?)));
        }
        if let Some(err) = dict.get_item("Err")? {
            return Ok(Outcome(Err(err
                .extract::<E>()
                .map_err(|e| -> PyErr { Into::<PyErr>::into(e) })?)));
        }
        Err(pyo3::exceptions::PyValueError::new_err(
            "expected a mapping with an 'Ok' or 'Err' key",
        ))
    }
}

/// `GeneralConfig` holds a `RosOpenCvIntrinsicsPod` and an `Isometry3`, both
/// foreign. Rather than wrap each, Python sees the flat wire form the device
/// actually exchanges, which carries the same information.
impl<'py> IntoPyObject<'py> for GeneralConfig {
    type Target = PyAny;
    type Output = Bound<'py, PyAny>;
    type Error = PyErr;

    fn into_pyobject(self, py: Python<'py>) -> PyResult<Self::Output> {
        use pyo3::IntoPyObjectExt;
        crate::wire::GeneralConfig::from(self).into_bound_py_any(py)
    }
}

impl<'a, 'py> FromPyObject<'a, 'py> for GeneralConfig {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        Ok(obj.extract::<crate::wire::GeneralConfig>()?.into())
    }
}

impl<'py> IntoPyObject<'py> for crate::Nothing {
    type Target = PyAny;
    type Output = Bound<'py, PyAny>;
    type Error = core::convert::Infallible;

    fn into_pyobject(self, py: Python<'py>) -> Result<Self::Output, Self::Error> {
        Ok(py.None().into_bound(py))
    }
}

impl<'a, 'py> FromPyObject<'a, 'py> for crate::Nothing {
    type Error = PyErr;

    fn extract(obj: Borrowed<'a, 'py, PyAny>) -> PyResult<Self> {
        if obj.is_none() {
            Ok(crate::Nothing)
        } else {
            Err(pyo3::exceptions::PyValueError::new_err("expected None"))
        }
    }
}
