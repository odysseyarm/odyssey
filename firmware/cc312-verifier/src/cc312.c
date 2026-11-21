#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "cc3xx.h"
#include "nrf_cc3xx_platform.h"
#include "psa/crypto.h"

extern int32_t cc312_oberon_hash_setup(bool supported);
extern int32_t cc312_oberon_hash_update(const uint8_t *input, size_t input_length);
extern int32_t cc312_oberon_hash_finish(uint8_t *hash, size_t hash_size,
                                        size_t *hash_length);
extern int32_t cc312_oberon_hash_abort(void);


psa_status_t psa_driver_wrapper_hash_compute(psa_algorithm_t alg,
                                              const uint8_t *input,
                                              size_t input_length,
                                              uint8_t *hash,
                                              size_t hash_size,
                                              size_t *hash_length)
{
    if (input_length != 0 && input == NULL) {
        return PSA_ERROR_INVALID_ARGUMENT;
    }

    psa_status_t status = cc312_oberon_hash_setup(alg == PSA_ALG_SHA_512);
    if (status == PSA_SUCCESS) {
        status = cc312_oberon_hash_update(input, input_length);
    }
    if (status == PSA_SUCCESS) {
        status = cc312_oberon_hash_finish(hash, hash_size, hash_length);
    }
    if (status != PSA_SUCCESS) {
        (void) cc312_oberon_hash_abort();
    }
    return status;
}

psa_status_t psa_driver_wrapper_hash_setup(psa_hash_operation_t *operation,
                                            psa_algorithm_t alg)
{
    if (operation == NULL) {
        return PSA_ERROR_INVALID_ARGUMENT;
    }
    return cc312_oberon_hash_setup(alg == PSA_ALG_SHA_512);
}

psa_status_t psa_driver_wrapper_hash_update(psa_hash_operation_t *operation,
                                             const uint8_t *input,
                                             size_t input_length)
{
    if (operation == NULL || (input_length != 0 && input == NULL)) {
        return PSA_ERROR_INVALID_ARGUMENT;
    }
    return cc312_oberon_hash_update(input, input_length);
}

psa_status_t psa_driver_wrapper_hash_finish(psa_hash_operation_t *operation,
                                             uint8_t *hash,
                                             size_t hash_size,
                                             size_t *hash_length)
{
    if (operation == NULL || hash_length == NULL) {
        return PSA_ERROR_INVALID_ARGUMENT;
    }
    return cc312_oberon_hash_finish(hash, hash_size, hash_length);
}

psa_status_t psa_driver_wrapper_hash_abort(psa_hash_operation_t *operation)
{
    if (operation == NULL) {
        return PSA_ERROR_INVALID_ARGUMENT;
    }
    return cc312_oberon_hash_abort();
}

int32_t cc312_verify_ed25519(const uint8_t *message, size_t message_length,
                             const uint8_t key[32], const uint8_t signature[64])
{
    if ((message_length != 0 && message == NULL) || key == NULL || signature == NULL) {
        return PSA_ERROR_INVALID_ARGUMENT;
    }

    bool owned_platform = !nrf_cc3xx_platform_is_initialized();
    if (owned_platform && nrf_cc3xx_platform_init_no_rng() != 0) {
        return PSA_ERROR_HARDWARE_FAILURE;
    }

    psa_key_attributes_t attributes = PSA_KEY_ATTRIBUTES_INIT;
    psa_set_key_type(&attributes,
                     PSA_KEY_TYPE_ECC_PUBLIC_KEY(PSA_ECC_FAMILY_TWISTED_EDWARDS));
    psa_set_key_bits(&attributes, 255);
    psa_set_key_usage_flags(&attributes, PSA_KEY_USAGE_VERIFY_MESSAGE);
    psa_set_key_algorithm(&attributes, PSA_ALG_PURE_EDDSA);

    psa_status_t status = cc3xx_internal_eddsa_verify(
        &attributes, key, 32, PSA_ALG_PURE_EDDSA, message, message_length,
        signature, 64);
    (void) cc312_oberon_hash_abort();

    if (owned_platform && nrf_cc3xx_platform_deinit() != 0) {
        return PSA_ERROR_HARDWARE_FAILURE;
    }
    return status;
}
